//! Server assembly and lifecycle.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use axum_server::accept::DefaultAcceptor;
use axum_server::tls_rustls::{RustlsAcceptor, RustlsConfig};
use hyper_util::rt::{TokioExecutor, TokioTimer};
use hyper_util::server::conn::auto::Builder;

use crate::api;
use crate::auth::ServerKey;
use crate::auth::keys::key_path;
use crate::config::Config;
use crate::conn_deadline::FirstByteDeadline;
use crate::db::Db;
use crate::error::{Result, ServerError};
use crate::state::AppState;
use crate::util::unix_now;

/// Grace period for in-flight requests on shutdown.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// Longest a connection may take to send request headers: its first byte,
/// each request's headers, and the wait for the next request on an idle
/// keep-alive connection.
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
        let tls = RustlsConfig::from_pem_file(
            state.config.security.tls_cert.trim(),
            state.config.security.tls_key.trim(),
        )
        .await
        .map_err(|error| ServerError::Config(format!("cannot load TLS material: {error}")))?;
        // Matches the HTTP/1.1-only server: never offer h2 in ALPN.
        let mut rustls = (*tls.get_inner()).clone();
        rustls.alpn_protocols = vec![b"http/1.1".to_vec()];
        tls.reload_from_config(std::sync::Arc::new(rustls));

        tracing::info!(%addr, "listening with direct TLS");
        let acceptor = FirstByteDeadline::new(RustlsAcceptor::new(tls), HEADER_READ_TIMEOUT);
        let mut server = axum_server::bind(addr).acceptor(acceptor).handle(handle);
        set_timeouts(server.http_builder());
        server.serve(service).await?;
    } else {
        tracing::info!(%addr, "listening on plain HTTP (terminate TLS at the proxy)");
        let acceptor = FirstByteDeadline::new(DefaultAcceptor, HEADER_READ_TIMEOUT);
        let mut server = axum_server::bind(addr).acceptor(acceptor).handle(handle);
        set_timeouts(server.http_builder());
        server.serve(service).await?;
    }
    Ok(())
}

/// Serves HTTP/1.1 only, with a header timeout.
///
/// Without a timer hyper enforces no header timeout at all, so a client
/// could hold a connection by never finishing its headers. HTTP/2 is left
/// out because detecting it means reading the connection's first bytes with
/// no timeout; the proxy and the clients all speak HTTP/1.1.
fn set_timeouts(builder: &mut Builder<TokioExecutor>) {
    let auto = std::mem::replace(builder, Builder::new(TokioExecutor::new()));
    *builder = auto.http1_only();
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
