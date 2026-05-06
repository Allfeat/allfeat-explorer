//! REST surface for the Nuxt frontend, mounted at `/api/v1`.
//!
//! Handlers are Axum-native (no `#[server]` macro). Each handler pulls
//! the shared [`AppState`] via `State`, resolves the network via
//! [`ChainCtx`], and delegates to `state.provider`. Errors map to
//! [`ApiError`], which owns the wire-shape JSON body and status code.
//!
//! The router is assembled in [`router`] and mounted once in `main.rs`.

pub mod accounts;
pub mod ats;
pub mod blocks;
pub mod cache_control;
pub mod error;
pub mod extrinsics;
pub mod indexing;
pub mod meta;
pub mod metadata;
pub mod networks;
pub mod runtime;
pub mod token;

use axum::middleware::from_fn;
use axum::routing::get;
use axum::Router;

use crate::network::{by_id, ChainCtx};
use crate::server::AppState;

pub use error::ApiError;

/// Build the REST router. Routes are grouped by `Cache-Control` tier so
/// the HTTP cache policy is declared where the routes are listed —
/// see [`cache_control`] for the tier semantics.
///
/// `AppState` is bound once at the end so the sub-routers stay typed as
/// `Router<AppState>` and the `State<AppState>` extractor keeps working
/// in every handler.
pub fn router(state: AppState) -> Router {
    // Listings + head-following feeds. Short TTL + big SWR.
    let latest: Router<AppState> = Router::new()
        .route(
            "/api/v1/networks/{network_id}/blocks",
            get(blocks::list_blocks),
        )
        .route(
            "/api/v1/networks/{network_id}/blocks/head",
            get(blocks::head_block),
        )
        .route(
            "/api/v1/networks/{network_id}/waveform",
            get(blocks::waveform_blocks),
        )
        .route(
            "/api/v1/networks/{network_id}/extrinsics",
            get(extrinsics::list_extrinsics),
        )
        .route(
            "/api/v1/networks/{network_id}/transfers",
            get(blocks::list_transfers),
        )
        .route(
            "/api/v1/networks/{network_id}/events",
            get(blocks::list_events),
        )
        .route(
            "/api/v1/networks/{network_id}/accounts",
            get(accounts::top_accounts),
        )
        .route(
            "/api/v1/networks/{network_id}/accounts/{address}",
            get(accounts::account_by_address),
        )
        .route(
            "/api/v1/networks/{network_id}/accounts/{address}/ats",
            get(accounts::account_ats),
        )
        .route(
            "/api/v1/networks/{network_id}/accounts/{address}/allocations",
            get(token::account_allocations),
        )
        .route(
            "/api/v1/networks/{network_id}/token/overview",
            get(token::token_overview),
        )
        .route("/api/v1/networks/{network_id}/ats", get(ats::list_ats))
        .route("/api/v1/networks/{network_id}/ats/feed", get(ats::ats_feed))
        .route(
            "/api/v1/networks/{network_id}/ats/stats",
            get(ats::ats_stats),
        )
        .layer(from_fn(cache_control::latest));

    // Detail-by-id — stable once the referenced block finalises.
    // `blocks/{number}` has to be more specific than `blocks/head`
    // already handled above; the fixed paths (`head`, `feed`, `stats`,
    // `extrinsics`, `events`) are all siblings under `latest` so the
    // catch-all `{number}` / `{index}` / `{envelope_id}` entries below
    // won't capture them.
    let detail: Router<AppState> = Router::new()
        .route(
            "/api/v1/networks/{network_id}/blocks/{number}",
            get(blocks::block_by_number),
        )
        .route(
            "/api/v1/networks/{network_id}/blocks/{number}/extrinsics",
            get(extrinsics::extrinsics_in_block),
        )
        .route(
            "/api/v1/networks/{network_id}/blocks/{number}/events",
            get(blocks::events_in_block),
        )
        .route(
            "/api/v1/networks/{network_id}/extrinsics/{id}",
            get(extrinsics::extrinsic_by_id),
        )
        .route(
            "/api/v1/networks/{network_id}/ats/{ats_id}",
            get(ats::ats_by_id),
        )
        .route(
            "/api/v1/networks/{network_id}/token/envelopes/{envelope_id}",
            get(token::envelope_detail),
        )
        .layer(from_fn(cache_control::detail));

    // Deploy / runtime-upgrade scoped. Minutes of browser cache are
    // fine — runtime upgrades ship on a scale of days to weeks.
    let static_: Router<AppState> = Router::new()
        .route("/api/v1/networks", get(networks::list_networks))
        .route(
            "/api/v1/networks/{network_id}/metadata/pallets",
            get(metadata::list_pallets),
        )
        .route(
            "/api/v1/networks/{network_id}/runtime",
            get(runtime::runtime_details),
        )
        .route(
            "/api/v1/networks/{network_id}/runtime/upgrades",
            get(runtime::runtime_upgrades),
        )
        .route(
            "/api/v1/networks/{network_id}/runtime/wasm",
            get(runtime::runtime_wasm),
        )
        .route(
            "/api/v1/networks/{network_id}/runtime/metadata",
            get(runtime::runtime_metadata),
        )
        .route("/api/v1/meta", get(meta::build_info))
        .layer(from_fn(cache_control::static_));

    // Indexer status feeds the UI banner; staleness would hide a stuck
    // indexer from operators and users alike.
    let no_store: Router<AppState> = Router::new()
        .route("/api/v1/indexing/status", get(indexing::indexer_status))
        .layer(from_fn(cache_control::no_store));

    Router::new()
        .merge(latest)
        .merge(detail)
        .merge(static_)
        .merge(no_store)
        .with_state(state)
}

/// Resolve a `network_id` path segment into a [`ChainCtx`] tied to the
/// server wall clock. Unknown ids are rejected at the router boundary
/// with a 404 so handlers never see a bogus network.
pub(super) fn ctx_for(network_id: &str) -> Result<ChainCtx, ApiError> {
    let spec = by_id(network_id)
        .ok_or_else(|| ApiError::NotFound(format!("unknown network '{network_id}'")))?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok(ChainCtx::new(spec, now_ms))
}

/// Validate a `count` query param against the endpoint's cap. Both
/// `0` and `count > max` surface as `400 Bad Request` — callers must
/// either get the range they asked for or a clear error, never a
/// silently trimmed window. `max` comes from each endpoint so small
/// payload endpoints (e.g. 72-bar waveform) can set their own ceiling.
pub(super) fn clamp_count(requested: u32, max: u32) -> Result<u32, ApiError> {
    if requested == 0 {
        return Err(ApiError::BadRequest("count must be >= 1".to_string()));
    }
    if requested > max {
        return Err(ApiError::BadRequest(format!(
            "count={requested} exceeds max={max}"
        )));
    }
    Ok(requested)
}
