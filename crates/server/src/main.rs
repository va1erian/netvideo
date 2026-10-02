//! Thin CLI entry point for `netvideo-server`.

#![forbid(unsafe_code)]

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use netvideo_server::{Config, audit, server};

#[derive(Debug, Parser)]
#[command(name = "netvideo-server", version, about)]
struct Cli {
    /// Path to `server.toml` (falls back to `NETVIDEO_CONFIG`).
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Subcommand; defaults to `serve`.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the HTTP(S) server.
    Serve,
    /// Generate a one-time pairing code and print it.
    Pair {
        /// Validity of the code, in seconds.
        #[arg(long, default_value_t = 600)]
        ttl: u64,
        /// Pair a viewer device instead of an administrator.
        #[arg(long)]
        viewer: bool,
    },
    /// List paired devices.
    Devices,
    /// Revoke a paired device by id.
    Revoke {
        /// The device id to revoke.
        device_id: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load_or_default(cli.config.as_deref()).context("loading configuration")?;

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("building async runtime")?;
            runtime
                .block_on(server::run(config))
                .context("running server")?;
        }
        Command::Pair { ttl, viewer } => {
            let _audit = audit::init(&config.server.data_dir).context("opening audit log")?;
            let ttl = ttl.min(netvideo_server::config::MAX_PAIRING_CODE_TTL_SECS);
            let code =
                server::print_pairing_code(&config, ttl, !viewer).context("generating code")?;
            audit::pairing_code_created("cli", None, !viewer);
            println!("{code}");
            let role = if viewer { "viewer" } else { "admin" };
            println!("Pairing code ({role}) valid for {ttl}s; enter it in the netvideo client.");
        }
        Command::Devices => {
            for device in server::list_devices(&config).context("listing devices")? {
                let status = if device.is_revoked {
                    "revoked"
                } else {
                    "active"
                };
                let last_seen = device
                    .last_seen
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "never".to_string());
                let role = if device.is_admin { "admin" } else { "viewer" };
                println!(
                    "{}  {:<20}  {:<8}  {:<6}  last_seen={last_seen}",
                    device.id, device.name, status, role
                );
            }
        }
        Command::Revoke { device_id } => {
            let _audit = audit::init(&config.server.data_dir).context("opening audit log")?;
            if server::revoke_device(&config, &device_id).context("revoking device")? {
                audit::device_revoked("cli", None, &device_id);
                println!("revoked {device_id}");
            } else {
                eprintln!("no such device: {device_id}");
                std::process::exit(1);
            }
        }
    }
    Ok(())
}
