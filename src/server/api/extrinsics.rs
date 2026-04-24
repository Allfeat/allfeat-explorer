//! Extrinsics handlers.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use crate::data::filters::ExtrinsicFilters;
use crate::domain::{CallResult, Extrinsic, Page, PageRequest};
use crate::server::api::{clamp_count, ctx_for, ApiError};
use crate::server::AppState;

const MAX_COUNT: u32 = 100;

#[derive(Deserialize)]
pub struct CountQuery {
    #[serde(default = "default_count")]
    pub count: u32,
}

// Fields are flat rather than `#[serde(flatten)]`-ing [`ExtrinsicFilters`]
// because `serde_urlencoded` + `flatten` silently fails to coerce
// `?signed=true` into a `bool` (it hands the inner struct a string via
// `MapAccess`). See `src/server/api/blocks.rs` for the same rationale.
#[derive(Deserialize)]
pub struct ListExtrinsicsQuery {
    #[serde(default = "default_count")]
    pub count: u32,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub signed: Option<bool>,
    #[serde(default)]
    pub pallet: Option<String>,
    #[serde(default)]
    pub call: Option<String>,
    /// `success` / `failed` (case-insensitive). `CallResult`'s default
    /// deserializer only accepts the JSON variants (`Success` /
    /// `Failed`), which looks wrong in a URL.
    #[serde(default, deserialize_with = "deserialize_call_result_opt")]
    pub status: Option<CallResult>,
}

fn default_count() -> u32 {
    25
}

fn deserialize_call_result_opt<'de, D>(d: D) -> Result<Option<CallResult>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<String> = Option::deserialize(d)?;
    match raw.as_deref() {
        None | Some("") => Ok(None),
        Some(s) if s.eq_ignore_ascii_case("success") => Ok(Some(CallResult::Success)),
        Some(s) if s.eq_ignore_ascii_case("failed") => Ok(Some(CallResult::Failed)),
        Some(other) => Err(serde::de::Error::custom(format!(
            "invalid status '{other}', expected 'success' or 'failed'"
        ))),
    }
}

pub async fn list_extrinsics(
    State(state): State<AppState>,
    Path(network_id): Path<String>,
    Query(query): Query<ListExtrinsicsQuery>,
) -> Result<Json<Page<Extrinsic>>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let count = clamp_count(query.count, MAX_COUNT)?;
    let req = PageRequest {
        count,
        cursor: query.cursor,
    };
    let filters = ExtrinsicFilters {
        signed: query.signed,
        pallet: query.pallet,
        call: query.call,
        status: query.status,
    };
    let page = state.provider.latest_extrinsics(ctx, req, filters).await?;
    Ok(Json(page))
}

pub async fn extrinsic_by_id(
    State(state): State<AppState>,
    Path((network_id, id)): Path<(String, String)>,
) -> Result<Json<Extrinsic>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let x = state
        .provider
        .extrinsic_by_id(ctx, &id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("extrinsic '{id}' not found")))?;
    Ok(Json(x))
}

pub async fn extrinsics_in_block(
    State(state): State<AppState>,
    Path((network_id, number)): Path<(String, u64)>,
) -> Result<Json<Vec<Extrinsic>>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let xs = state.provider.extrinsics_in_block(ctx, number).await?;
    Ok(Json(xs))
}
