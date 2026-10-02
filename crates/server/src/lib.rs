//! `netvideo-server`: a hardened home-lab video server.
//!
//! The crate is split into the configuration loader, the SQLite-backed store,
//! the authentication subsystem, the library scanner, the HTTP API and the
//! security primitives.
//! [`run`] wires them together; `main.rs` is a thin CLI around it.

#![forbid(unsafe_code)]

pub mod api;
pub mod audit;
pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod scanner;
pub mod security;
pub mod server;
pub mod state;
pub mod util;

pub use config::Config;
pub use error::{Result, ServerError};
pub use server::{build_state, run, serve};
