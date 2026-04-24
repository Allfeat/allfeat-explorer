//! Blocks + transfers handlers.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::data::filters::{BlockFilters, EventFilters, TransferFilters};
use crate::domain::{Block, BlockEvent, Page, PageRequest, Transfer, WaveformBlock};
use crate::server::api::{clamp_count, ctx_for, ApiError};
use crate::server::AppState;

/// Listing-endpoint cap. Requests asking for more than this return
/// `400 Bad Request` — explorers rarely want more than a screenful and
/// a silent clamp would mislead callers about what they actually got.
const MAX_COUNT: u32 = 100;

// ── Query-struct shape ────────────────────────────────────────────────
//
// Filter fields are **inlined** into each query struct rather than
// pulled in via `#[serde(flatten)]`. `serde_urlencoded` (what axum's
// `Query` extractor uses) silently fails to coerce string→primitive
// through a flattened sub-struct — `?signed=true` would decode `signed`
// as the string `"true"` and bail with "expected bool". Keeping the
// fields here, flat, keeps the wire-level parsing correct; the handler
// body re-assembles the typed [`BlockFilters`] on the way into the
// provider.

#[derive(Deserialize)]
pub struct ListBlocksQuery {
    #[serde(default = "default_count")]
    pub count: u32,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub finalized: Option<bool>,
    #[serde(default)]
    pub min_extrinsics: Option<u32>,
}

#[derive(Deserialize)]
pub struct CountQuery {
    #[serde(default = "default_count")]
    pub count: u32,
}

#[derive(Serialize)]
pub struct HeadResponse {
    #[serde(serialize_with = "crate::serde_helpers::u64_string::serialize")]
    pub number: u64,
}

fn default_count() -> u32 {
    25
}

pub async fn list_blocks(
    State(state): State<AppState>,
    Path(network_id): Path<String>,
    Query(query): Query<ListBlocksQuery>,
) -> Result<Json<Page<Block>>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let count = clamp_count(query.count, MAX_COUNT)?;
    let req = PageRequest {
        count,
        cursor: query.cursor,
    };
    let filters = BlockFilters {
        finalized: query.finalized,
        min_extrinsics: query.min_extrinsics,
    };
    let page = state.provider.latest_blocks(ctx, req, filters).await?;
    Ok(Json(page))
}

pub async fn head_block(
    State(state): State<AppState>,
    Path(network_id): Path<String>,
) -> Result<Json<HeadResponse>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let number = state.provider.head_block(ctx).await?;
    Ok(Json(HeadResponse { number }))
}

pub async fn block_by_number(
    State(state): State<AppState>,
    Path((network_id, number)): Path<(String, u64)>,
) -> Result<Json<Block>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let block = state
        .provider
        .block_by_number(ctx, number)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("block {number} not found")))?;
    Ok(Json(block))
}

/// Every event emitted in the block — including Initialization and
/// Finalization phases. The Events tab on the block detail page consumes
/// this directly so session-change effects stay visible.
pub async fn events_in_block(
    State(state): State<AppState>,
    Path((network_id, number)): Path<(String, u64)>,
) -> Result<Json<Vec<BlockEvent>>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let events = state.provider.events_in_block(ctx, number).await?;
    Ok(Json(events))
}

/// Lean block window for the home-page waveform hero. Returns the
/// newest `count` blocks (default 72 — same as the hero's bar count)
/// projected to [`WaveformBlock`], shaving the per-bar payload to the
/// fields the hero actually reads. The hero never paginates, so the
/// `Page<Block>` envelope is flattened back into a `Vec<WaveformBlock>`
/// here — no cursor is exposed on the wire.
pub async fn waveform_blocks(
    State(state): State<AppState>,
    Path(network_id): Path<String>,
    Query(query): Query<CountQuery>,
) -> Result<Json<Vec<WaveformBlock>>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let count = clamp_count(query.count, MAX_COUNT)?;
    let req = PageRequest {
        count,
        cursor: None,
    };
    let page = state
        .provider
        .latest_blocks(ctx, req, BlockFilters::default())
        .await?;
    Ok(Json(
        page.items.into_iter().map(WaveformBlock::from).collect(),
    ))
}

#[derive(Deserialize)]
pub struct ListTransfersQuery {
    #[serde(default = "default_count")]
    pub count: u32,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
}

pub async fn list_transfers(
    State(state): State<AppState>,
    Path(network_id): Path<String>,
    Query(query): Query<ListTransfersQuery>,
) -> Result<Json<Page<Transfer>>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let count = clamp_count(query.count, MAX_COUNT)?;
    let req = PageRequest {
        count,
        cursor: query.cursor,
    };
    let filters = TransferFilters {
        from: query.from,
        to: query.to,
    };
    let page = state.provider.latest_transfers(ctx, req, filters).await?;
    Ok(Json(page))
}

#[derive(Deserialize)]
pub struct ListEventsQuery {
    #[serde(default = "default_count")]
    pub count: u32,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub pallet: Option<String>,
    #[serde(default)]
    pub variant: Option<String>,
}

/// Chain-wide latest events feed. Unlike [`events_in_block`], this one
/// walks back from the tip and flattens events across blocks, so the
/// dashboard's Events tab shows session-change effects from
/// `Initialization` and `Finalization` phases alongside the
/// extrinsic-attached events.
pub async fn list_events(
    State(state): State<AppState>,
    Path(network_id): Path<String>,
    Query(query): Query<ListEventsQuery>,
) -> Result<Json<Page<BlockEvent>>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let count = clamp_count(query.count, MAX_COUNT)?;
    let req = PageRequest {
        count,
        cursor: query.cursor,
    };
    let filters = EventFilters {
        pallet: query.pallet,
        variant: query.variant,
    };
    let page = state.provider.latest_events(ctx, req, filters).await?;
    Ok(Json(page))
}
