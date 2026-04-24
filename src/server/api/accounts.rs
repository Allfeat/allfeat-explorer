//! Accounts + per-account ATS handlers.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use crate::data::filters::AccountAtsFilters;
use crate::domain::{Account, AtsRecord, Page, PageRequest};
use crate::server::api::{clamp_count, ctx_for, ApiError};
use crate::server::AppState;

const MAX_ACCOUNTS: u32 = 100;
const MAX_ATS_PER_ACCOUNT: u32 = 100;

#[derive(Deserialize)]
pub struct TopAccountsQuery {
    #[serde(default = "default_top_count")]
    pub count: u32,
}

#[derive(Deserialize)]
pub struct AccountAtsQuery {
    #[serde(default = "default_ats_limit")]
    pub count: u32,
    #[serde(default)]
    pub cursor: Option<String>,
}

fn default_top_count() -> u32 {
    20
}

fn default_ats_limit() -> u32 {
    10
}

pub async fn top_accounts(
    State(state): State<AppState>,
    Path(network_id): Path<String>,
    Query(query): Query<TopAccountsQuery>,
) -> Result<Json<Page<Account>>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let count = clamp_count(query.count, MAX_ACCOUNTS)?;
    // Top-accounts stays "give me the top-N by balance" on the data
    // layer — there's no cursor contract for it. Wrap the plain `Vec`
    // in a `Page<Account>` so the HTTP surface is uniform with every
    // other list endpoint. `total` is left `None` (a global COUNT(*)
    // against the accounts table grows with adoption and the UI
    // doesn't render it for the top-N view).
    let items = state.provider.top_accounts(ctx, count).await?;
    Ok(Json(Page {
        items,
        page_info: crate::domain::PageInfo {
            total: None,
            next_cursor: None,
            has_more: false,
        },
    }))
}

pub async fn account_by_address(
    State(state): State<AppState>,
    Path((network_id, address)): Path<(String, String)>,
) -> Result<Json<Account>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let account = state
        .provider
        .account_by_address(ctx, &address)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("account '{address}' not found")))?;
    Ok(Json(account))
}

pub async fn account_ats(
    State(state): State<AppState>,
    Path((network_id, address)): Path<(String, String)>,
    Query(query): Query<AccountAtsQuery>,
) -> Result<Json<Page<AtsRecord>>, ApiError> {
    let ctx = ctx_for(&network_id)?;
    let count = clamp_count(query.count, MAX_ATS_PER_ACCOUNT)?;
    let req = PageRequest {
        count,
        cursor: query.cursor,
    };
    let page = state
        .provider
        .account_ats(ctx, &address, req, AccountAtsFilters::default())
        .await?;
    Ok(Json(page))
}
