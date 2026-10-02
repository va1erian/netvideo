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
        security: SecurityConfig::default(),
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
