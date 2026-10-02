//! Structured audit logging for security-relevant events.
//!
//! Operational logs go to stdout; audit events additionally go to a JSON log
//! file under the data directory, one JSON object per line, so they can be
//! shipped or grepped without parsing prose. The helper functions here are
//! deliberately typed: it is impossible to emit an audit event with the wrong
//! field set.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Tracing target reserved for audit events.
pub const AUDIT_TARGET: &str = "netvideo_server::audit";

/// Initializes logging. Keep the returned guard alive for the process
/// lifetime; dropping it stops the audit writer.
pub fn init(data_dir: &Path) -> std::io::Result<WorkerGuard> {
    std::fs::create_dir_all(data_dir)?;
    let file_appender = tracing_appender::rolling::daily(data_dir, "audit.log");
    // Security events must not be dropped under load: block the audit event
    // producer rather than lose a record (`lossy(false)`).
    let (audit_writer, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .lossy(false)
        .finish(file_appender);

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,netvideo_server=debug"));

    // `RUST_LOG` only tunes the console: a global filter would also drop
    // audit events when an operator quiets the console.
    // stderr, so CLI output on stdout (a pairing code) stays clean.
    let console = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true)
        .compact()
        .with_filter(filter);
    let audit = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(audit_writer)
        .with_target(false)
        .with_current_span(false)
        .with_filter(EnvFilter::new(format!("{AUDIT_TARGET}=info")));

    tracing_subscriber::registry()
        .with(console)
        .with(audit)
        .init();
    Ok(guard)
}

/// A failed authentication attempt: bad token, revoked device or bad proof.
pub fn auth_failed(client_ip: &str, reason: &str) {
    tracing::warn!(target: AUDIT_TARGET, event = "auth_failed", client_ip, reason);
}

/// A failed pairing attempt (wrong, expired or reused code).
pub fn pair_failed(client_ip: &str) {
    tracing::warn!(target: AUDIT_TARGET, event = "pair_failed", client_ip);
}

/// A device successfully paired.
pub fn device_paired(client_ip: &str, device_id: &str, device_name: &str) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "device_paired",
        client_ip,
        device_id,
        device_name
    );
}

/// A pairing code was minted, from the CLI (`client_ip` is `"cli"`) or by
/// an admin device.
pub fn pairing_code_created(client_ip: &str, device_id: Option<&str>, admin: bool) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "pairing_code_created",
        client_ip,
        device_id,
        admin
    );
}

/// A device was revoked, from the CLI (`by_device` is `None`) or by an
/// admin device.
pub fn device_revoked(client_ip: &str, by_device: Option<&str>, device_id: &str) {
    tracing::warn!(
        target: AUDIT_TARGET,
        event = "device_revoked",
        client_ip,
        by_device,
        device_id
    );
}

/// A token was refreshed after a successful proof of possession.
pub fn token_refreshed(client_ip: &str, device_id: &str) {
    tracing::info!(target: AUDIT_TARGET, event = "token_refreshed", client_ip, device_id);
}

/// A stored path failed the library-root jail.
pub fn path_violation(client_ip: &str, device_id: &str, video_id: &str) {
    tracing::warn!(
        target: AUDIT_TARGET,
        event = "path_violation",
        client_ip,
        device_id,
        video_id
    );
}

/// A request was rejected by a rate limiter.
pub fn rate_limited(client_ip: &str, scope: &str) {
    tracing::warn!(target: AUDIT_TARGET, event = "rate_limited", client_ip, scope);
}

/// A library scan finished.
pub fn scan_finished(stats: &crate::scanner::ScanStats, elapsed_ms: u64) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "scan_finished",
        videos_found = stats.videos_found,
        written = stats.written,
        removed = stats.removed,
        probe_failures = stats.probe_failures,
        partial = stats.partial,
        elapsed_ms
    );
}
