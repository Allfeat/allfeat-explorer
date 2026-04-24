//! Token allocation handlers — overview, per-envelope detail, per-account
//! allocations. All routes are mainnet-only at the data layer; non-mainnet
//! networks surface as 404 via [`DataError::NotSupported`].

use axum::extract::{Path, State};
use axum::Json;

use crate::domain::{Allocation, EnvelopeDetail, EnvelopeId, TokenOverview};
use crate::server::api::{ctx_for, ApiError};
use crate::server::AppState;

pub async fn token_overview(
    State(state): State<AppState>,
    Path(network_id): Path<String>,
) -> Result<Json<TokenOverview>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let overview = state.provider.token_overview(ctx).await?;
    Ok(Json(overview))
}

pub async fn envelope_detail(
    State(state): State<AppState>,
    Path((network_id, envelope_id)): Path<(String, String)>,
) -> Result<Json<EnvelopeDetail>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let id = EnvelopeId::from_slug(&envelope_id)
        .ok_or_else(|| ApiError::NotFound(format!("unknown envelope '{envelope_id}'")))?;
    let detail = state
        .provider
        .envelope_detail(ctx, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("envelope '{envelope_id}' not found")))?;
    Ok(Json(detail))
}

pub async fn account_allocations(
    State(state): State<AppState>,
    Path((network_id, address)): Path<(String, String)>,
) -> Result<Json<Vec<Allocation>>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let xs = state.provider.account_allocations(ctx, &address).await?;
    Ok(Json(xs))
}
