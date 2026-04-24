//! Server-side wiring: config parsing, AppState construction, health
//! probes, Prometheus metrics, and the `/api/v1/*` REST surface.

pub mod api;
pub mod config;
pub mod db;
pub mod health;
#[cfg(not(feature = "mock"))]
pub mod metrics;
pub mod state;

pub use config::{ConfigError, ServerConfig};
pub use db::{run_migrations, MIGRATOR};
pub use state::AppState;
