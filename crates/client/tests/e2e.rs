//! End-to-end tests: the client against a real `netvideo-server` served
//! over a loopback TCP socket.

use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::sync::Arc;

use netvideo_client::{ClientError, CredentialStore, FileStore, ServerEndpoint, Session};
use netvideo_server::config::{Config, LibraryConfig, SecurityConfig, ServerConfig};
use netvideo_server::state::AppState;
use netvideo_server::util::unix_now;
use tempfile::TempDir;

struct Server {
    state: AppState,
    url: String,
    dir: TempDir,
}

/// Starts a scanned server on a loopback port.
fn start_server() -> Server {
    start_server_with(SecurityConfig::default())
}

fn start_server_with(security: SecurityConfig) -> Server {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("library");
    std::fs::create_dir_all(root.join("Films")).expect("library dir");
    std::fs::write(root.join("Films/heat.mkv"), b"0123456789").expect("video");
    std::fs::write(root.join("clip.mp4"), b"clip").expect("video");

    let config = Config {
        server: ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            data_dir: dir.path().join("data"),
            trusted_proxies: vec![],
        },
        security,
        library: LibraryConfig {
            paths: vec![root],
            scan_interval_secs: 0,
            ffprobe_path: Path::new("/nonexistent/ffprobe").into(),
        },
    };
    let state = netvideo_server::build_state(config).expect("build state");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let addr = listener.local_addr().expect("addr");
    let app = netvideo_server::api::router(state.clone());
    let scan_state = state.clone();
    let (ready, scanned) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("server runtime");
        runtime.block_on(async move {
            scan_state
                .scan
                .scan_once(&scan_state.db)
                .await
                .expect("scan");
            ready.send(()).expect("ready");
            let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("serve");
        });
    });
    scanned.recv().expect("scanned");
    Server {
        state,
        url: format!("http://{addr}"),
        dir,
    }
}

impl Server {
    fn code(&self, admin: bool) -> String {
        netvideo_server::auth::pairing::generate_pairing_code(
            &self.state.db,
            &self.state.keys,
            600,
            admin,
            unix_now(),
        )
        .expect("code")
    }

    fn session(&self, store: &Arc<FileStore>) -> Session {
        let endpoint = ServerEndpoint::new("Home", &self.url).expect("endpoint");
        Session::new(endpoint, store.clone()).expect("session")
    }
}

#[test]
fn pairs_browses_and_saves_progress() {
    let server = start_server();
    let store = Arc::new(FileStore::with_dir(server.dir.path().join("client")));
    let session = server.session(&store);
    assert!(matches!(session.roots(), Err(ClientError::NotPaired)));
    assert!(matches!(
        session.pair("000000", "phone"),
        Err(ClientError::PairingCode)
    ));
    let device = session.pair(&server.code(false), "phone").expect("pair");
    assert_eq!(session.device_id().as_deref(), Some(device.as_str()));

    let roots = session.roots().expect("roots");
    assert_eq!(roots.len(), 1);
    let page = session.folder(&roots[0].id, None, 1).expect("page");
    assert_eq!(page.folders[0].name, "Films");
    assert!(page.videos.is_empty());
    let cursor = page.next_cursor.expect("second page");
    let page = session
        .folder(&roots[0].id, Some(&cursor), 10)
        .expect("page 2");
    let clip = &page.videos[0];
    assert_eq!(clip.name, "clip.mp4");
    assert!(clip.progress.is_none());

    session
        .save_progress(&clip.id, 1_500, false)
        .expect("progress");
    let detail = session.video(&clip.id).expect("detail");
    assert_eq!(detail.mime, "video/mp4");
    assert_eq!(detail.progress.expect("progress").position_ms, 1_500);
    assert!(
        session
            .file_url(&clip.id)
            .ends_with(&format!("/videos/{}/file", clip.id))
    );

    // A new session (an app restart) picks the stored credentials up.
    let restarted = server.session(&store);
    assert_eq!(restarted.roots().expect("roots").len(), 1);
}

#[test]
fn refreshes_a_token_near_expiry() {
    let server = start_server();
    let store = Arc::new(FileStore::with_dir(server.dir.path().join("client")));
    let session = server.session(&store);
    session.pair(&server.code(false), "phone").expect("pair");

    let id = session.endpoint().id.clone();
    let mut stored = store.load(&id).unwrap().unwrap();
    let old_token = stored.token.clone();
    stored.expires_at = unix_now() + 60;
    store.save(&id, &stored).unwrap();

    let session = server.session(&store);
    let token = session.token().expect("refreshed");
    assert_ne!(token, old_token);
    assert!(store.load(&id).unwrap().unwrap().expires_at > unix_now() + 3600);
    assert_eq!(session.token().expect("cached"), token, "no second refresh");
}

#[test]
fn revoked_devices_get_unauthorized_and_can_forget() {
    let server = start_server();
    let store = Arc::new(FileStore::with_dir(server.dir.path().join("client")));
    let session = server.session(&store);
    let device = session.pair(&server.code(true), "laptop").expect("pair");
    server.state.db.revoke_device(&device).expect("revoke");
    assert!(matches!(session.roots(), Err(ClientError::Unauthorized)));

    session.forget().expect("forget");
    assert!(store.load(&session.endpoint().id).unwrap().is_none());
    assert!(matches!(session.roots(), Err(ClientError::NotPaired)));
}

#[test]
fn health_reports_ok() {
    let server = start_server();
    let health = server.session(&Arc::new(FileStore::with_dir(server.dir.path().join("c"))));
    assert_eq!(health.client().health().expect("health").status, "ok");
}

#[test]
fn the_server_accepts_client_proofs() {
    use netvideo_client::auth;
    use netvideo_client::session::{PROOF_SKEW, PROOF_TTL};
    use netvideo_server::auth::paseto::verify_refresh_proof;

    let (secret, public) = auth::generate_keypair().expect("keypair");
    let fingerprint = auth::token_fingerprint("some.token");
    let proof =
        auth::issue_refresh_proof(&secret, &fingerprint, PROOF_TTL, PROOF_SKEW).expect("proof");
    let verified = verify_refresh_proof(&public, &proof, &fingerprint).expect("accepted");
    assert!(!verified.proof_id.is_empty());
    assert!(verify_refresh_proof(&public, &proof, &auth::token_fingerprint("other")).is_err());
}

#[test]
fn short_lived_tokens_are_not_refreshed_on_every_call() {
    let server = start_server_with(SecurityConfig {
        token_ttl_hours: 24,
        ..SecurityConfig::default()
    });
    let store = Arc::new(FileStore::with_dir(server.dir.path().join("client")));
    let session = server.session(&store);
    session.pair(&server.code(false), "phone").expect("pair");
    let first = session.token().expect("token");
    session.roots().expect("roots");
    assert_eq!(session.token().expect("token"), first);
}

#[test]
fn a_failed_refresh_keeps_the_current_token() {
    let server = start_server();
    let store = Arc::new(FileStore::with_dir(server.dir.path().join("client")));
    let session = server.session(&store);
    let device = session.pair(&server.code(false), "phone").expect("pair");
    let id = session.endpoint().id.clone();
    let mut stored = store.load(&id).unwrap().unwrap();
    stored.expires_at = unix_now() + 60;
    store.save(&id, &stored).unwrap();

    // The server refuses the refresh: the still-valid token is kept.
    server.state.db.revoke_device(&device).expect("revoke");
    let session = server.session(&store);
    assert_eq!(session.token().expect("fallback"), stored.token);

    // Once it expires there is nothing to fall back on.
    stored.expires_at = unix_now() - 1;
    store.save(&id, &stored).unwrap();
    let session = server.session(&store);
    assert!(matches!(session.token(), Err(ClientError::Unauthorized)));
}

#[test]
fn viewers_forget_locally_even_though_the_server_refuses_revocation() {
    let server = start_server();
    let store = Arc::new(FileStore::with_dir(server.dir.path().join("client")));
    let session = server.session(&store);
    session.pair(&server.code(false), "tv").expect("pair");
    session.forget().expect("forget");
    assert!(store.load(&session.endpoint().id).unwrap().is_none());
}

#[test]
fn pins_the_server_key_from_a_pairing_link() {
    let server = start_server();
    let store = Arc::new(FileStore::with_dir(server.dir.path().join("client")));
    let session = server.session(&store);
    let real = server.state.keys.fingerprint().expect("fingerprint");

    let wrong = "0".repeat(64);
    assert!(matches!(
        session.pair_pinned(&server.code(false), "phone", Some(&wrong)),
        Err(ClientError::ServerKey)
    ));
    assert!(
        session.device_id().is_none(),
        "nothing stored on a mismatch"
    );

    session
        .pair_pinned(&server.code(false), "phone", Some(&real))
        .expect("pair");
    let id = session.endpoint().id.clone();
    let stored = store.load(&id).unwrap().unwrap();
    assert_eq!(
        stored
            .server_key
            .as_deref()
            .map(netvideo_client::auth::server_key_fingerprint),
        Some(real)
    );
}

#[test]
fn unpinned_credentials_pin_the_key_on_refresh() {
    let server = start_server();
    let store = Arc::new(FileStore::with_dir(server.dir.path().join("client")));
    let session = server.session(&store);
    session.pair(&server.code(false), "phone").expect("pair");
    let id = session.endpoint().id.clone();
    let mut stored = store.load(&id).unwrap().unwrap();
    stored.server_key = None;
    stored.expires_at = unix_now() + 60;
    store.save(&id, &stored).unwrap();

    let session = server.session(&store);
    assert_ne!(session.token().expect("refreshed"), stored.token);
    let pinned = store
        .load(&id)
        .unwrap()
        .unwrap()
        .server_key
        .expect("pinned");
    assert_eq!(
        netvideo_client::auth::server_key_fingerprint(&pinned),
        server.state.keys.fingerprint().unwrap()
    );
}

#[test]
fn tokens_from_another_key_are_refused_on_refresh() {
    let server = start_server();
    let store = Arc::new(FileStore::with_dir(server.dir.path().join("client")));
    let session = server.session(&store);
    session.pair(&server.code(false), "phone").expect("pair");

    // Pin some other server's key, then make the token due for refresh.
    let (_, other) = netvideo_client::auth::generate_keypair().unwrap();
    let id = session.endpoint().id.clone();
    let mut stored = store.load(&id).unwrap().unwrap();
    stored.server_key = Some(netvideo_client::auth::public_key_paserk(&other).unwrap());
    stored.expires_at = unix_now() + 60;
    store.save(&id, &stored).unwrap();

    // The renewed token fails the pin: an impostor is assumed, so the
    // session stops instead of handing it the current token again.
    let session = server.session(&store);
    assert!(matches!(session.token(), Err(ClientError::ServerKey)));
    assert_eq!(store.load(&id).unwrap().unwrap().token, stored.token);
}

#[test]
fn a_reply_for_a_substituted_device_key_is_refused() {
    use netvideo_client::auth;
    let server = start_server();
    // A relaying man-in-the-middle redeems the code with its own key and
    // hands the genuine reply to the victim.
    let (_, attacker) = auth::generate_keypair().unwrap();
    let attacker = auth::public_key_paserk(&attacker).unwrap();
    let endpoint = ServerEndpoint::new("Home", &server.url).unwrap();
    let client = netvideo_client::RemoteClient::from_endpoint(&endpoint).unwrap();
    let reply = client
        .pair(&server.code(true), "phone", &attacker)
        .expect("pair");
    let server_key = reply.server_key.expect("server key");

    let (_, victim) = auth::generate_keypair().unwrap();
    let victim = auth::device_key_fingerprint(&auth::public_key_paserk(&victim).unwrap());
    let check = |device_key: &str| {
        auth::verify_issued_token(&server_key, &reply.auth_token, &reply.device_id, device_key)
    };
    assert!(matches!(check(&victim), Err(ClientError::ServerKey)));
    assert!(check(&auth::device_key_fingerprint(&attacker)).is_ok());
}

#[test]
fn fingerprints_match_the_servers() {
    use netvideo_client::auth;
    let (_, public) = auth::generate_keypair().unwrap();
    let paserk = auth::public_key_paserk(&public).unwrap();
    assert_eq!(
        auth::device_key_fingerprint(&paserk),
        netvideo_server::auth::paseto::device_key_fingerprint(&paserk)
    );
    assert_eq!(
        auth::server_key_fingerprint(&paserk),
        netvideo_server::auth::keys::server_key_fingerprint(&paserk)
    );
}
