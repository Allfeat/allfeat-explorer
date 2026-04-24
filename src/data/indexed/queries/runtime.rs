//! SQL reads backing the runtime-history endpoint.
//!
//! One query today — the upgrade timeline. Every indexed block already
//! carries the active `spec_version` at index time (see
//! [`crate::indexer::projections::blocks::BlockRow`]), so deriving the
//! upgrade timeline is an aggregate over `blocks` rather than a
//! dedicated `runtime_upgrades` index. The `runtime_versions` table in
//! the migration blueprint (§1 of `docs/indexing-plan.md`) will replace
//! this aggregate when Phase 2 starts persisting metadata blobs, but
//! until then the `blocks.spec_version` column is the source of truth.

use sqlx::PgPool;

use crate::data::error::{DataError, DataResult};
use crate::domain::RuntimeUpgrade;

/// Distinct `spec_version` values observed in the indexed history for
/// `network_sid`, each paired with the **first** block at which it was
/// seen and that block's wall-clock timestamp. Newest-first ordering so
/// the "most recent upgrade" lands at index 0 in the UI's timeline.
///
/// `is_current` is populated by the caller — this query is
/// chain-identity-agnostic and can't decide on its own which row is
/// the live spec. The handler passes the currently-running
/// `spec_version` (from the RPC `runtime_identity()` read) and flips
/// the flag post-query.
pub async fn runtime_upgrades(pool: &PgPool, network_sid: i16) -> DataResult<Vec<RuntimeUpgrade>> {
    // `DISTINCT ON (spec_version)` + `ORDER BY spec_version, num ASC`
    // gives us the block at which each version was first observed,
    // which is exactly what we need — every subsequent block on the
    // same spec is already accounted for by the grouping. The outer
    // ORDER BY then flips the list into newest-first so the UI
    // renders recent upgrades at the top without a reverse on the
    // frontend side.
    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT spec_version, first_block, first_block_ts FROM ( \
           SELECT DISTINCT ON (spec_version) \
             spec_version, num AS first_block, timestamp_ms AS first_block_ts \
           FROM blocks \
           WHERE network_id = $1 \
           ORDER BY spec_version, num ASC \
         ) t \
         ORDER BY first_block DESC",
    )
    .bind(network_sid)
    .fetch_all(pool)
    .await
    .map_err(|e| DataError::Rpc(format!("runtime_upgrades(net={network_sid}): {e}")))?;

    Ok(rows
        .into_iter()
        .map(|(spec_version, first_block, ts)| {
            let block = first_block.max(0) as u64;
            // Genesis has no `timestamp.set` inherent → the indexer
            // writes `timestamp_ms = 0`. Flipping that back to `None`
            // here stops the frontend from rendering a 1970-era date;
            // real post-genesis blocks keep their recorded timestamp.
            let ts_opt = if ts > 0 { Some(ts) } else { None };
            RuntimeUpgrade {
                spec_version: spec_version.max(0) as u32,
                first_block: Some(block),
                first_block_timestamp_ms: ts_opt,
                // `is_current` is overwritten by the handler once the
                // live spec is known; leaving it `false` here prevents
                // a stale history row from pretending to be live if
                // the handler forgets to reconcile.
                is_current: false,
            }
        })
        .collect())
}
