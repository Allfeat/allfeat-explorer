//! `GET /api/v1/meta` — build identity of the running backend.
//!
//! The UI footer consumes this to display which commit is in
//! production. Crate version comes from `CARGO_PKG_VERSION`; the git
//! short sha is stamped in by `build.rs` into `GIT_SHA` and falls back
//! to `"unknown"` when the build happened outside a git checkout.

use axum::Json;
use serde::Serialize;

#[cfg(feature = "ts-bindings")]
use ts_rs::TS;

#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, Serialize)]
pub struct BuildInfo {
    /// Crate version from `Cargo.toml`.
    pub version: &'static str,
    /// Short git sha of the HEAD commit the binary was built from.
    pub git_sha: &'static str,
}

pub async fn build_info() -> Json<BuildInfo> {
    Json(BuildInfo {
        version: env!("CARGO_PKG_VERSION"),
        git_sha: env!("GIT_SHA"),
    })
}
