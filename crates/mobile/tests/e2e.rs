//! Pairing, browsing and playback headers through `MobileSession`, against
//! a real `netvideo-server` on a loopback socket.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::sync::{Arc, Mutex};

use netvideo_mobile::{MobileError, MobileSession, SecretVault};
use netvideo_server::config::{Config, LibraryConfig, SecurityConfig, ServerConfig};
use netvideo_server::util::unix_now;

#[derive(Default)]
struct MemoryVault(Mutex<HashMap<String, String>>);

impl SecretVault for MemoryVault {
    fn read(&self, key: String) -> Result<Option<String>, MobileError> {
        Ok(self.0.lock().unwrap().get(&key).cloned())
    }
    fn write(&self, key: String, value: String) -> Result<(), MobileError> {
        self.0.lock().unwrap().insert(key, value);
        Ok(())
    }
    fn delete(&self, key: String) -> Result<(), MobileError> {
        self.0.lock().unwrap().remove(&key);
        Ok(())
    }
}

#[test]
fn pairs_browses_and_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("library");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("clip.mp4"), b"clip").unwrap();
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
    let state = netvideo_server::build_state(config).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = netvideo_server::api::router(state.clone());
    let scan = state.clone();
    let (ready, scanned) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            scan.scan.scan_once(&scan.db).await.unwrap();
            ready.send(()).unwrap();
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let service = app.into_make_service_with_connect_info::<SocketAddr>();
            axum::serve(listener, service).await.unwrap();
        });
    });
    scanned.recv().unwrap();

    let session = MobileSession::new(url, Arc::new(MemoryVault::default())).unwrap();
    session.check_server().unwrap();
    assert!(matches!(
        session.pair("000000".into(), "phone".into(), None),
        Err(MobileError::PairingCode)
    ));
    let code = netvideo_server::auth::pairing::generate_pairing_code(
        &state.db,
        &state.keys,
        600,
        false,
        unix_now(),
    )
    .unwrap();
    let link = netvideo_mobile::parse_pairing_link(format!(
        "netvideo://pair?v=1&url=x%3A&code={code}&key={}",
        state.keys.fingerprint().unwrap()
    ));
    assert!(link.is_err(), "the URL must be http(s)");
    let fingerprint = state.keys.fingerprint().unwrap();
    session
        .pair(code, "phone".into(), Some(fingerprint))
        .unwrap();
    assert!(session.is_paired());

    let roots = session.roots().unwrap();
    let page = session.folder(roots[0].id.clone(), None, 50).unwrap();
    let clip = &page.videos[0];
    session
        .save_progress(clip.id.clone(), 42_000, false)
        .unwrap();
    let detail = session.video(clip.id.clone()).unwrap();
    assert_eq!(detail.progress.unwrap().position_ms, 42_000);
    assert!(
        session
            .authorization()
            .unwrap()
            .starts_with("Bearer v4.public.")
    );

    let missing = session.video("0".repeat(32)).expect_err("missing");
    assert!(
        matches!(missing, MobileError::Server { status: 404, .. }),
        "{missing:?}"
    );
}
