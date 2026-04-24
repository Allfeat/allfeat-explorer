//! Indexer status handler — consumed by the UI `IndexingBanner`.
//!
//! Thin Axum wrapper over [`crate::server::health::collect_indexer_status`]
//! so the pure derivation stays testable without spinning up the HTTP
//! stack.

use axum::extract::State;
use axum::Json;

use crate::server::api::ApiError;
use crate::server::health::{collect_indexer_status, IndexerStatus};
use crate::server::AppState;

pub async fn indexer_status(
    State(state): State<AppState>,
) -> Result<Json<Vec<IndexerStatus>>, ApiError> {
    Ok(Json(collect_indexer_status(&state).await))
}
