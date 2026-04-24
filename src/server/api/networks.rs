//! `GET /api/v1/networks` — list the networks this deployment serves.

use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::network::NetworkSpec;
use crate::server::api::ApiError;
use crate::server::AppState;

#[derive(Serialize)]
pub struct NetworksResponse {
    pub networks: Vec<NetworkSpec>,
}

/// Return the enabled networks. In mock builds that's every entry in
/// [`NETWORKS`] with each spec's hardcoded defaults; in RPC builds it's
/// the subset whose `RPC_ENDPOINT_<ID>` env var was set at boot (order
/// preserved from the static catalogue), with each spec's `block_time_secs`,
/// `spec_name`, and `spec_version` replaced by the live values read from
/// the chain (`Timestamp::MinimumPeriod` constant, `Core_version` runtime
/// API). Both RPC reads are cached per client, so this stays cheap per
/// request. The SS58 prefix stays on the hardcoded `NetworkSpec` value —
/// it's effectively immutable per chain and we don't pay a round-trip for
/// it. Failures fall back to the hardcoded defaults — a slightly stale
/// header beats a blank network list.
///
/// The enriched `spec_name` goes through a `Box::leak` so it can flow
/// through the otherwise-`Copy` `NetworkSpec` struct without an
/// owned-string refactor. Memory cost is bounded by the number of
/// distinct spec names the explorer ever sees (one or two per process)
/// and the same leaked slice is reused across requests for the same
/// network — subsequent calls pull the cached `RuntimeIdentity` and
/// reuse the `&'static` pointer from the first fill.
pub async fn list_networks(
    State(state): State<AppState>,
) -> Result<Json<NetworksResponse>, ApiError> {
    #[cfg(feature = "mock")]
    let networks: Vec<NetworkSpec> = {
        let _ = state; // no runtime state needed in mock
        crate::network::NETWORKS.iter().map(|n| **n).collect()
    };

    #[cfg(not(feature = "mock"))]
    let networks: Vec<NetworkSpec> = {
        let mut out = Vec::with_capacity(state.indexed_network_ids.len());
        for id in state.indexed_network_ids.iter() {
            let Some(spec) = crate::network::by_id(id) else {
                continue;
            };
            let mut spec = *spec;
            if let Some(client) = state.indexer_clients.get(id) {
                if let Ok(secs) = client.block_time_secs().await {
                    spec.block_time_secs = secs;
                }
                if let Ok(identity) = client.runtime_identity(None).await {
                    // Static defaults on `NetworkSpec` still hold the
                    // declaration-time `spec_name`; comparing here avoids
                    // leaking the same name twice when the chain agrees
                    // with the static catalogue (the common case).
                    spec.spec_name = if identity.spec_name == spec.spec_name {
                        spec.spec_name
                    } else {
                        Box::leak(identity.spec_name.into_boxed_str())
                    };
                    spec.spec_version = identity.spec_version;
                }
            }
            out.push(spec);
        }
        out
    };

    Ok(Json(NetworksResponse { networks }))
}
