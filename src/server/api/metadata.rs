//! Chain metadata handlers.
//!
//! These endpoints surface static runtime schema the UI needs to build
//! filter controls — they're not paginated and don't depend on the
//! provider. The data comes straight from the compiled `ALLFEAT_METADATA`
//! blob, so the handler cost is a single `Vec<String>` allocation.

use axum::extract::Path;
use axum::Json;
use serde::Serialize;

use crate::data::metadata::callable_pallet_names;
use crate::server::api::{ctx_for, ApiError};

#[derive(Serialize)]
pub struct PalletsResponse {
    pub pallets: Vec<String>,
}

/// Return the pallet names that declare at least one callable extrinsic.
/// Used by the extrinsics list page to populate a pallet filter
/// dropdown that matches the values actually stored in
/// `extrinsics.pallet`.
///
/// The `network_id` is validated for parity with the rest of the REST
/// surface; the payload itself is static because every supported
/// network currently ships the same runtime metadata blob.
pub async fn list_pallets(
    Path(network_id): Path<String>,
) -> Result<Json<PalletsResponse>, ApiError> {
    let _ctx = ctx_for(&network_id)?;
    Ok(Json(PalletsResponse {
        pallets: callable_pallet_names(),
    }))
}
