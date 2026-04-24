//! ATS (pallet-ats) handlers: list, detail, feed, stats.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use crate::data::filters::{AtsFeedFilters, AtsFilters};
use crate::domain::{AtsFeedItem, AtsRecord, AtsStats, Page, PageRequest};
use crate::server::api::{clamp_count, ctx_for, ApiError};
use crate::server::AppState;

const MAX_COUNT: u32 = 100;

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_count")]
    pub count: u32,
    #[serde(default)]
    pub cursor: Option<String>,
}

fn default_count() -> u32 {
    25
}

pub async fn list_ats(
    State(state): State<AppState>,
    Path(network_id): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<AtsRecord>>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let count = clamp_count(query.count, MAX_COUNT)?;
    let req = PageRequest {
        count,
        cursor: query.cursor,
    };
    let page = state
        .provider
        .ats_list(ctx, req, AtsFilters::default())
        .await?;
    Ok(Json(page))
}

pub async fn ats_by_index(
    State(state): State<AppState>,
    Path((network_id, index)): Path<(String, u32)>,
) -> Result<Json<AtsRecord>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let record = state
        .provider
        .ats_by_index(ctx, index)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("ATS #{index} not found")))?;
    Ok(Json(record))
}

pub async fn ats_feed(
    State(state): State<AppState>,
    Path(network_id): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<AtsFeedItem>>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let count = clamp_count(query.count, MAX_COUNT)?;
    let req = PageRequest {
        count,
        cursor: query.cursor,
    };
    let page = state
        .provider
        .ats_version_feed(ctx, req, AtsFeedFilters::default())
        .await?;
    Ok(Json(page))
}

pub async fn ats_stats(
    State(state): State<AppState>,
    Path(network_id): Path<String>,
) -> Result<Json<AtsStats>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let stats = state.provider.ats_stats(ctx).await?;
    Ok(Json(stats))
}
