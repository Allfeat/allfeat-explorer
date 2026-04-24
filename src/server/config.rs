//! Runtime server configuration sourced from environment variables.
//!
//! Parsed once at boot, then held in [`crate::server::state::AppState`]. The
//! aim is one authoritative place where ops knobs live; nothing else should
//! call `std::env::var` at request time.
//!
//! The mock ↔ RPC choice is compile-time (feature = "mock") — there is no
//! runtime switch. RPC endpoints are read from env vars and gate whether a
//! network is even enabled in this deployment.
//!
//! ## Variables
//!
//! | Variable                   | Meaning                                              |
//! |----------------------------|------------------------------------------------------|
//! | `RPC_ENDPOINT_<NETWORK>`   | Endpoint for `NetworkSpec::id`. **Unset = the network is disabled** (hidden from the UI, not indexed). |
//! | `LISTEN_ADDR`              | `host:port` the HTTP server binds. Default `127.0.0.1:8088`. |
//! | `WS_ALLOWED_ORIGINS`       | Comma-separated origin allowlist for `/api/v1/live`. `*` = any. |
//! | `API_ALLOWED_ORIGINS`      | Comma-separated CORS allowlist for `/api/v1/*` REST. `*` = any. |
//! | `DATABASE_URL`             | Postgres connection string for the indexer. Unset = no DB (RPC-only fallback). |
//! | `INDEXER_BACKFILL_CONCURRENCY` | Number of parallel backfill workers. Default 8, minimum 1. |
//!
//! Networks are opt-in: if `RPC_ENDPOINT_<ID>` is not set for a given entry
//! in `crate::network::NETWORKS`, that network is treated as disabled and
//! never reaches the UI, the provider, or the indexer. Boot aborts if no
//! network at all has an endpoint configured in a non-mock build — there
//! would be nothing to serve.
//!
//! `WS_ALLOWED_ORIGINS` and `API_ALLOWED_ORIGINS` default to `*` so local
//! dev workflows (`bun run dev:api` + Nuxt proxy) keep working without
//! extra setup; prod deployments **must** set both explicitly (see the
//! boot-time WARN in `main.rs`).
//!
//! `LISTEN_ADDR` defaults to `127.0.0.1:8088` so the Nuxt dev server on
//! `:3000` can proxy `/api/*` to it without a port collision.
//!
//! `DATABASE_URL` is optional during the indexer rollout (Phases 0–6): when
//! absent, the server keeps serving everything via `RpcProvider` and the
//! indexer workers never spawn. Phase 7 flips `--mode=server` to refuse
//! booting without it.

use std::net::SocketAddr;

#[cfg(not(feature = "mock"))]
use std::collections::HashMap;

#[cfg(not(feature = "mock"))]
use crate::network::NETWORKS;

/// Default bind address when `LISTEN_ADDR` is unset. Loopback because a
/// reverse proxy / Nuxt dev proxy is expected in front; `:8088` leaves
/// `:3000` free for the Nuxt SSR process.
const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:8088";

#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// RPC endpoint per `NetworkSpec::id`. Empty in `mock` builds — the mock
    /// provider never opens a connection.
    #[cfg(not(feature = "mock"))]
    pub rpc_endpoints: HashMap<&'static str, String>,

    /// Socket the HTTP server binds to. Defaults to `127.0.0.1:8088`.
    pub listen_addr: SocketAddr,

    /// Origins allowed to upgrade a `/api/v1/live` WebSocket connection.
    /// Special value `*` disables the check entirely (dev default). In
    /// prod set this to an exact match list like
    /// `["https://explorer.allfeat.com"]` — anything else is rejected
    /// with `403 Forbidden` before the upgrade.
    pub ws_allowed_origins: Vec<String>,

    /// CORS allowlist for `/api/v1/*` REST. Same shape and defaults as
    /// `ws_allowed_origins`; `*` disables the check entirely.
    pub api_allowed_origins: Vec<String>,

    /// Postgres connection string for the indexer. `None` disables the
    /// indexer entirely and routes every read through `RpcProvider` — the
    /// pre-Phase-0 behaviour, kept available for dev machines without a
    /// Postgres running.
    pub database_url: Option<String>,

    /// How many backfill workers run in parallel. Clamped to ≥1 at
    /// parse time so a stray `INDEXER_BACKFILL_CONCURRENCY=0` never
    /// wedges the queue. Defaults to 8 — double the plan's original
    /// §7 Phase 2 recommendation (4) after the archive node showed
    /// headroom on the ~20 h Melodie backfill budget.
    pub indexer_backfill_concurrency: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Non-mock build booted with no `RPC_ENDPOINT_<ID>` set for any network
    /// declared in `crate::network::NETWORKS`. There is nothing to index or
    /// serve, so the process aborts rather than starting an explorer with
    /// zero chains behind it.
    #[error(
        "no RPC endpoint configured: set at least one RPC_ENDPOINT_<NETWORK> env var \
         (expected one of: {0})"
    )]
    NoNetworksConfigured(String),

    /// `LISTEN_ADDR` didn't parse as `host:port`.
    #[error("invalid LISTEN_ADDR: {0}")]
    InvalidListenAddr(String),
}

impl ServerConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let listen_addr = parse_listen_addr(std::env::var("LISTEN_ADDR").ok())?;
        let ws_allowed_origins = parse_origins(std::env::var("WS_ALLOWED_ORIGINS").ok());
        let api_allowed_origins = parse_origins(std::env::var("API_ALLOWED_ORIGINS").ok());
        let database_url = std::env::var("DATABASE_URL").ok().filter(|s| !s.is_empty());
        let indexer_backfill_concurrency =
            parse_concurrency(std::env::var("INDEXER_BACKFILL_CONCURRENCY").ok());

        #[cfg(not(feature = "mock"))]
        {
            let rpc_endpoints =
                collect_rpc_endpoints(|key| std::env::var(key).ok().filter(|s| !s.is_empty()))?;

            Ok(Self {
                rpc_endpoints,
                listen_addr,
                ws_allowed_origins,
                api_allowed_origins,
                database_url,
                indexer_backfill_concurrency,
            })
        }

        #[cfg(feature = "mock")]
        {
            Ok(Self {
                listen_addr,
                ws_allowed_origins,
                api_allowed_origins,
                database_url,
                indexer_backfill_concurrency,
            })
        }
    }

    /// True when every request passes the origin check — the dev default,
    /// and the state we warn about at boot.
    pub fn ws_origin_wildcard(&self) -> bool {
        self.ws_allowed_origins.iter().any(|s| s == "*")
    }

    /// True when CORS accepts any origin. Inverted from the WS check:
    /// for REST we *want* wildcard in dev so the Nuxt proxy works with
    /// zero config; prod must narrow it.
    pub fn api_origin_wildcard(&self) -> bool {
        self.api_allowed_origins.iter().any(|s| s == "*")
    }
}

/// Parse `LISTEN_ADDR` into a `SocketAddr`. Invalid values abort boot
/// rather than falling back to the default — a typo in the deploy
/// manifest should fail loudly, not silently bind the wrong port.
fn parse_listen_addr(raw: Option<String>) -> Result<SocketAddr, ConfigError> {
    let raw = raw.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let value = raw.unwrap_or(DEFAULT_LISTEN_ADDR);
    value
        .parse::<SocketAddr>()
        .map_err(|e| ConfigError::InvalidListenAddr(format!("{value}: {e}")))
}

/// Parse `WS_ALLOWED_ORIGINS` into a trimmed, non-empty list. Unset or empty
/// collapses to `["*"]` so dev workflows keep working; prod opts in via a
/// comma-separated value (`https://a.example,https://b.example`).
fn parse_origins(raw: Option<String>) -> Vec<String> {
    match raw {
        Some(s) => {
            let list: Vec<String> = s
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if list.is_empty() {
                vec!["*".to_string()]
            } else {
                list
            }
        }
        None => vec!["*".to_string()],
    }
}

#[cfg(not(feature = "mock"))]
fn endpoint_var(network_id: &str) -> String {
    format!("RPC_ENDPOINT_{}", network_id.to_ascii_uppercase())
}

/// Collect enabled endpoints from a caller-provided lookup (usually
/// `std::env::var`, swapped out by tests). Fails when no network has an
/// endpoint configured so the caller can abort boot with an actionable
/// message.
#[cfg(not(feature = "mock"))]
fn collect_rpc_endpoints(
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<HashMap<&'static str, String>, ConfigError> {
    let mut rpc_endpoints = HashMap::new();
    for net in NETWORKS {
        let key = endpoint_var(net.id);
        if let Some(url) = lookup(&key) {
            rpc_endpoints.insert(net.id, url);
        }
    }

    if rpc_endpoints.is_empty() {
        let expected: Vec<String> = NETWORKS.iter().map(|n| endpoint_var(n.id)).collect();
        return Err(ConfigError::NoNetworksConfigured(expected.join(", ")));
    }

    Ok(rpc_endpoints)
}

/// Parse `INDEXER_BACKFILL_CONCURRENCY` with a floor of 1 and a sane
/// default. An unparseable or unset value falls back to `DEFAULT`
/// rather than silently disabling the pool; an explicit `0` is
/// clamped up to 1 so a typo never wedges the queue.
fn parse_concurrency(raw: Option<String>) -> usize {
    const DEFAULT: usize = 8;
    raw.as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<usize>().ok())
        .map(|v| v.max(1))
        .unwrap_or(DEFAULT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrency_defaults_when_unset_or_empty() {
        assert_eq!(parse_concurrency(None), 8);
        assert_eq!(parse_concurrency(Some(String::new())), 8);
        assert_eq!(parse_concurrency(Some("   ".to_string())), 8);
    }

    #[test]
    fn concurrency_clamps_zero_up_and_falls_back_on_garbage() {
        // Explicit 0 clamps up to the minimum viable pool (1 worker) —
        // silently dropping to 0 would wedge the queue.
        assert_eq!(parse_concurrency(Some("0".to_string())), 1);
        // Garbage falls back to the documented default so a typo in
        // the env var doesn't turn into a confusing missing-worker bug.
        assert_eq!(parse_concurrency(Some("not-a-number".to_string())), 8);
    }

    #[test]
    fn concurrency_accepts_positive_integers() {
        assert_eq!(parse_concurrency(Some("1".to_string())), 1);
        assert_eq!(parse_concurrency(Some("12".to_string())), 12);
    }

    #[cfg(not(feature = "mock"))]
    #[test]
    fn endpoints_skip_unset_networks() {
        // Only ALLFEAT has an endpoint — MELODIE should not land in the map.
        let endpoints = collect_rpc_endpoints(|key| {
            (key == "RPC_ENDPOINT_ALLFEAT").then(|| "ws://127.0.0.1:9944".to_string())
        })
        .expect("at least one endpoint configured");
        assert!(endpoints.contains_key("allfeat"));
        assert!(!endpoints.contains_key("melodie"));
    }

    #[cfg(not(feature = "mock"))]
    #[test]
    fn endpoints_empty_is_boot_error() {
        // Zero configured networks → actionable error, not silent boot.
        let err = collect_rpc_endpoints(|_| None).expect_err("should refuse empty config");
        let msg = err.to_string();
        assert!(msg.contains("RPC_ENDPOINT_ALLFEAT"), "got: {msg}");
        assert!(msg.contains("RPC_ENDPOINT_MELODIE"), "got: {msg}");
    }
}
