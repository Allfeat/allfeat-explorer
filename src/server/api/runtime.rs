//! Runtime-identity handlers.
//!
//! Four endpoints back the `/runtime` page:
//!
//! * `GET /api/v1/networks/{id}/runtime[?at=N]` — point-in-time snapshot
//!   of `Core_version`, the `:code` fingerprint, the genesis hash, and
//!   the compile-time metadata version. `?at=N` shifts the read to
//!   block `N` instead of the finalized head.
//! * `GET /api/v1/networks/{id}/runtime/upgrades` — historical timeline
//!   of distinct `spec_version` values. Indexed networks return the
//!   full list aggregated from `blocks`; RPC-only networks return a
//!   single "current" entry.
//! * `GET /api/v1/networks/{id}/runtime/wasm[?at=N]` — raw `:code`
//!   blob streamed as `application/wasm`. Feeds the "Download .wasm"
//!   button on the runtime page.
//! * `GET /api/v1/networks/{id}/runtime/metadata` — compile-time
//!   SCALE-encoded metadata blob (`RuntimeMetadataPrefixed`) streamed
//!   as `application/octet-stream`. Feeds the "Raw metadata" button.
//!
//! The JSON endpoints aren't paginated — the upgrade list is bounded
//! by the number of runtime versions that ever shipped on the chain
//! (single digits in practice) and the details response is a single
//! record. The two binary endpoints return a raw body with an
//! `attachment` disposition so browsers save-as rather than navigate.

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::domain::{RuntimeDetails, RuntimeUpgrade};
use crate::server::api::{ctx_for, ApiError};
use crate::server::AppState;

#[derive(Deserialize)]
pub struct RuntimeQuery {
    /// Optional block number to snapshot against. Absent means
    /// "latest finalized". Callers that need the exact runtime running
    /// at a historical block pass this to anchor the `:code` and
    /// `Core_version` reads to that block's pin — same as polkadot.js's
    /// "at block" control.
    #[serde(default)]
    pub at: Option<u64>,
}

#[derive(Serialize)]
pub struct RuntimeResponse {
    pub runtime: RuntimeDetails,
}

#[derive(Serialize)]
pub struct RuntimeUpgradesResponse {
    pub upgrades: Vec<RuntimeUpgrade>,
}

/// Return the runtime snapshot for `network_id` at `?at=N` (or the
/// finalized head). A block number past the current chain tip surfaces
/// as `400 Bad Request` rather than a generic RPC failure — the
/// provider layer owns that check.
pub async fn runtime_details(
    State(state): State<AppState>,
    Path(network_id): Path<String>,
    Query(query): Query<RuntimeQuery>,
) -> Result<Json<RuntimeResponse>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let runtime = state.provider.runtime_details(ctx, query.at).await?;
    Ok(Json(RuntimeResponse { runtime }))
}

/// Return the historical `spec_version` timeline for `network_id`,
/// newest-first. One row per distinct version observed in the indexed
/// history; RPC-only networks surface a single "current" row with
/// `first_block = 0` (the frontend interprets that sentinel as
/// "deployment block unknown").
pub async fn runtime_upgrades(
    State(state): State<AppState>,
    Path(network_id): Path<String>,
) -> Result<Json<RuntimeUpgradesResponse>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let upgrades = state.provider.runtime_upgrades(ctx).await?;
    Ok(Json(RuntimeUpgradesResponse { upgrades }))
}

/// Stream the runtime WASM blob for `network_id` at `?at=N` (or the
/// finalized head). Bypasses the `ChainData` trait — the blob only
/// exists on the RPC side and a mock equivalent would be misleading
/// (the button carries the expectation of a real runtime artifact).
/// Returns `503 Service Unavailable` on mock builds or networks with
/// no configured endpoint so the frontend can disable the button
/// instead of downloading an error JSON.
pub async fn runtime_wasm(
    State(state): State<AppState>,
    Path(network_id): Path<String>,
    Query(query): Query<RuntimeQuery>,
) -> Result<Response, ApiError> {
    // Validate the network id the same way the JSON handlers do so
    // unknown ids surface as a clean 404 before we touch any provider.
    let _ctx = ctx_for(&network_id)?;

    #[cfg(feature = "mock")]
    {
        let _ = (state, query);
        Err(ApiError::NotFound(format!(
            "WASM download is unavailable in mock mode for '{network_id}'"
        )))
    }

    #[cfg(not(feature = "mock"))]
    {
        let client = state
            .indexer_clients
            .get(network_id.as_str())
            .ok_or_else(|| {
                ApiError::NotFound(format!("network '{network_id}' is not configured"))
            })?;
        let bytes = client.runtime_wasm_bytes(query.at).await?;

        // Encode a content-disposition that both helps a browser save
        // with a sensible name and, when `?at=N` is present, makes the
        // snapshot block visible in the filename. Bytes are plain UTF-8,
        // all-ASCII — no RFC 5987 encoding dance needed.
        let filename = match query.at {
            Some(n) => format!("{network_id}-runtime-{n}.wasm"),
            None => format!("{network_id}-runtime.wasm"),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/wasm"),
        );
        headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
                .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
        );
        // axum's `IntoResponse` for `(StatusCode, HeaderMap, Vec<u8>)`
        // streams the body verbatim with the supplied headers. Vec<u8>
        // vs a streaming body doesn't matter at these sizes (few MB
        // max) and the simpler type keeps the handler obvious.
        Ok((StatusCode::OK, headers, bytes).into_response())
    }
}

/// Return the compile-time SCALE-encoded runtime metadata blob as
/// `application/octet-stream`. The response is always the same for a
/// given build — we ship one metadata artifact per deployment and
/// every supported network decodes against it. No `?at=` support:
/// historical metadata would require per-spec blobs the indexer
/// hasn't captured yet (`runtime_versions` table exists in the schema
/// but is unpopulated).
pub async fn runtime_metadata(
    State(_state): State<AppState>,
    Path(network_id): Path<String>,
) -> Result<Response, ApiError> {
    let _ctx = ctx_for(&network_id)?;
    let blob: &'static [u8] = crate::data::metadata::METADATA_BYTES;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"{network_id}-metadata.scale\""
        ))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    Ok((StatusCode::OK, headers, blob.to_vec()).into_response())
}
