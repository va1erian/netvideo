//! Server assembly and lifecycle.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use axum_server::tls_rustls::RustlsConfig;
use hyper_util::rt::{TokioExecutor, TokioTimer};
use hyper_util::server::conn::auto::Builder;

use crate::api;
use crate::auth::ServerKey;
use crate::auth::keys::key_path;
use crate::config::Config;
use crate::db::Db;
use crate::error::{Result, ServerError};
use crate::state::AppState;
use crate::util::unix_now;

/// Grace period for in-flight requests on shutdown.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// Longest a connection may take to send request headers. hyper starts the
/// timer when a connection opens and again whenever a keep-alive connection
/// goes idle, so it also closes silent and idle connections.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Opens the store and loads the server key, returning shared state ready to
/// serve. Does not start listening.
pub fn build_state(config: Config) -> Result<AppState> {
    let db = Db::open(&config.server.data_dir)?;
    let keys = ServerKey::load_or_create(&key_path(&config.server.data_dir))?;
    AppState::new(config, db, keys)
}

/// Runs the HTTP(S) server until `shutdown` resolves.
pub async fn serve(
    state: AppState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let addr = listen_addr(&state.config)?;
    let app = api::router(state.clone());

    state.scan.trigger(state.db.clone());
    state.scan.start_periodic(
        state.db.clone(),
        Duration::from_secs(state.config.library.scan_interval_secs),
    );

    let handle = axum_server::Handle::new();
    let shutdown_handle = handle.clone();
    tokio::spawn(async move {
        shutdown.await;
        shutdown_handle.graceful_shutdown(Some(SHUTDOWN_GRACE));
    });
    let service = app.into_make_service_with_connect_info::<SocketAddr>();

    if state.config.tls_enabled() {
        let tls = load_tls(&state.config).await?;
        tracing::info!(%addr, "listening with direct TLS");
        let mut server = axum_server::bind_rustls(addr, tls)
            .handle(handle)
            .http1_only();
        set_header_timeout(server.http_builder());
        server.serve(service).await?;
    } else {
        tracing::info!(%addr, "listening on plain HTTP (terminate TLS at the proxy)");
        let mut server = axum_server::bind(addr).handle(handle).http1_only();
        set_header_timeout(server.http_builder());
        server.serve(service).await?;
    }
    Ok(())
}

/// Loads the TLS certificate and key, offering only HTTP/1.1 in ALPN.
///
/// axum-server's PEM loaders (including its reload helpers) always offer
/// h2, so any future certificate reload must go through this function.
async fn load_tls(config: &Config) -> Result<RustlsConfig> {
    let tls = RustlsConfig::from_pem_file(
        config.security.tls_cert.trim(),
        config.security.tls_key.trim(),
    )
    .await
    .map_err(|error| ServerError::Config(format!("cannot load TLS material: {error}")))?;
    let mut rustls = (*tls.get_inner()).clone();
    rustls.alpn_protocols = vec![b"http/1.1".to_vec()];
    tls.reload_from_config(std::sync::Arc::new(rustls));
    Ok(tls)
}

/// Enables hyper's header timeout. Without a timer hyper enforces none, so a
/// client could hold a connection by never finishing its headers.
///
/// The server is HTTP/1.1 only: detecting HTTP/2 means reading a
/// connection's first bytes with no timeout, and HTTP/2 connections have no
/// header timeout at all. The proxy and the clients all speak HTTP/1.1.
fn set_header_timeout(builder: &mut Builder<TokioExecutor>) {
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT);
}

/// Initializes logging, builds state and serves until a shutdown signal.
pub async fn run(config: Config) -> Result<()> {
    let _guard = crate::audit::init(&config.server.data_dir)?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "netvideo-server starting"
    );
    let state = build_state(config)?;
    serve(state, shutdown_signal()).await
}

/// Resolves the configured host/port into a socket address. `server.host`
/// must be an IP literal (validated by [`Config::validate`]).
pub fn listen_addr(config: &Config) -> Result<SocketAddr> {
    let host = config.server.host.trim();
    host.parse::<IpAddr>()
        .map(|ip| SocketAddr::new(ip, config.server.port))
        .map_err(|_| ServerError::Config(format!("server.host {host:?} is not an IP address")))
}

/// Resolves on Ctrl-C (all platforms) or SIGTERM (Unix).
pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutdown signal received");
}

/// Generates a pairing code, for the `pair` CLI subcommand. The operator's
/// own codes grant admin unless `grants_admin` is false.
pub fn print_pairing_code(config: &Config, ttl_secs: u64, grants_admin: bool) -> Result<String> {
    let db = Db::open(&config.server.data_dir)?;
    let key = ServerKey::load_or_create(&key_path(&config.server.data_dir))?;
    let code =
        crate::auth::pairing::generate_pairing_code(&db, &key, ttl_secs, grants_admin, unix_now())?;
    Ok(code)
}

/// The terminal QR code for a pairing `code`, for `pair --qr`.
pub fn pairing_qr(config: &Config, url: &str, code: &str) -> Result<String> {
    let key = ServerKey::load_or_create(&key_path(&config.server.data_dir))?;
    let link = crate::auth::link::pairing_link(url, code, &key.fingerprint()?)?;
    crate::auth::link::render_qr(&link)
}

/// Lists paired devices, for the `devices` CLI subcommand.
pub fn list_devices(config: &Config) -> Result<Vec<crate::db::models::Device>> {
    let db = Db::open(&config.server.data_dir)?;
    db.list_devices()
}

/// Revokes a device, for the `revoke` CLI subcommand.
pub fn revoke_device(config: &Config, id: &str) -> Result<bool> {
    let db = Db::open(&config.server.data_dir)?;
    db.revoke_device(id)
}
