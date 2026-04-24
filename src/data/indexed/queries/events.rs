//! SQL reads that project the `events` table back into the
//! lightweight [`EventRef`] pairs the UI renders.
//!
//! The extrinsic read path needs events for three different shapes:
//!
//! * **One extrinsic** (detail page) — `events_for_extrinsic`.
//! * **One block's worth** (block detail + `extrinsics_in_block`) —
//!   `events_by_ext_idx_in_block` does a single WHERE scan and
//!   groups in memory, avoiding an N+1 over each extrinsic.
//! * **A heterogeneous list** across many blocks (`latest_extrinsics`)
//!   — `events_for_pairs` passes two parallel arrays through
//!   `unnest` and probes `events_phase_idx` per pair in one round
//!   trip.
//!
//! Every query filters `phase_kind = 0` (ApplyExtrinsic). Block-init
//! and finalization events are not attached to any extrinsic and the
//! UI doesn't render them here — a future "block events" tab would
//! read them directly, not through this module.

use std::collections::HashMap;

use sqlx::PgPool;

use crate::data::error::{DataError, DataResult};
use crate::data::metadata::decode_event_fields;
use crate::domain::EventRef;
use crate::indexer::projections::events::PHASE_APPLY_EXTRINSIC;

/// Apply-extrinsic events for `(network_id, block_num)`, bucketed by
/// the extrinsic index they attach to.
///
/// One round-trip + O(n) grouping: the caller hydrates every
/// extrinsic in the block from one query instead of one per row.
/// Uses the `events_phase_idx` prefix `(network_id, block_num,
/// phase_kind)` for the scan.
pub async fn events_by_ext_idx_in_block(
    pool: &PgPool,
    network_sid: i16,
    block_num: u64,
    ss58_prefix: u16,
) -> DataResult<HashMap<u32, Vec<EventRef>>> {
    let rows: Vec<(i32, String, String, Vec<u8>)> = sqlx::query_as(
        "SELECT phase_idx, pallet, variant, data_scale \
           FROM events \
          WHERE network_id = $1 \
            AND block_num = $2 \
            AND phase_kind = $3 \
            AND phase_idx IS NOT NULL \
          ORDER BY idx ASC",
    )
    .bind(network_sid)
    .bind(block_num as i64)
    .bind(PHASE_APPLY_EXTRINSIC)
    .fetch_all(pool)
    .await
    .map_err(|e| {
        DataError::Rpc(format!(
            "events_by_ext_idx_in_block(net={network_sid}/{block_num}): {e}"
        ))
    })?;

    let mut out: HashMap<u32, Vec<EventRef>> = HashMap::new();
    for (phase_idx, pallet, variant, data_scale) in rows {
        let ext = phase_idx.max(0) as u32;
        let fields = decode_event_fields(&pallet, &variant, &data_scale, ss58_prefix);
        out.entry(ext).or_default().push(EventRef {
            module: pallet,
            name: variant,
            fields,
        });
    }
    Ok(out)
}

/// Events belonging to a single extrinsic, in on-chain order. Same
/// `EventRef` shape the RPC mapper produces so downstream components
/// don't care which backend served the read.
pub async fn events_for_extrinsic(
    pool: &PgPool,
    network_sid: i16,
    block_num: u64,
    ext_idx: u32,
    ss58_prefix: u16,
) -> DataResult<Vec<EventRef>> {
    let rows: Vec<(String, String, Vec<u8>)> = sqlx::query_as(
        "SELECT pallet, variant, data_scale \
           FROM events \
          WHERE network_id = $1 \
            AND block_num = $2 \
            AND phase_kind = $3 \
            AND phase_idx = $4 \
          ORDER BY idx ASC",
    )
    .bind(network_sid)
    .bind(block_num as i64)
    .bind(PHASE_APPLY_EXTRINSIC)
    .bind(ext_idx as i32)
    .fetch_all(pool)
    .await
    .map_err(|e| {
        DataError::Rpc(format!(
            "events_for_extrinsic(net={network_sid}/{block_num},{ext_idx}): {e}"
        ))
    })?;
    Ok(rows
        .into_iter()
        .map(|(module, name, data_scale)| {
            let fields = decode_event_fields(&module, &name, &data_scale, ss58_prefix);
            EventRef {
                module,
                name,
                fields,
            }
        })
        .collect())
}

/// Bulk-fetch events for many `(block_num, ext_idx)` pairs in one
/// round-trip. Implements the "heterogeneous blocks" path used by
/// `latest_extrinsics`, where naive per-extrinsic queries would be
/// N+1.
///
/// The JOIN uses `unnest` over two parallel arrays — Postgres treats
/// the result as a virtual table of tuples, and the
/// `events_phase_idx` btree handles the equality lookup on both
/// `(block_num, phase_idx)` sides. Callers key the returned map by
/// the same `(block_num, ext_idx)` pair they sent.
pub async fn events_for_pairs(
    pool: &PgPool,
    network_sid: i16,
    pairs: &[(u64, u32)],
    ss58_prefix: u16,
) -> DataResult<HashMap<(u64, u32), Vec<EventRef>>> {
    if pairs.is_empty() {
        return Ok(HashMap::new());
    }
    let block_nums: Vec<i64> = pairs.iter().map(|(b, _)| *b as i64).collect();
    let ext_idxs: Vec<i32> = pairs.iter().map(|(_, i)| *i as i32).collect();

    let rows: Vec<(i64, i32, String, String, Vec<u8>)> = sqlx::query_as(
        "SELECT e.block_num, e.phase_idx, e.pallet, e.variant, e.data_scale \
           FROM events e \
           JOIN unnest($2::BIGINT[], $3::INT[]) AS r(block_num, ext_idx) \
             ON e.block_num = r.block_num AND e.phase_idx = r.ext_idx \
          WHERE e.network_id = $1 AND e.phase_kind = $4 \
          ORDER BY e.block_num DESC, e.idx ASC",
    )
    .bind(network_sid)
    .bind(&block_nums)
    .bind(&ext_idxs)
    .bind(PHASE_APPLY_EXTRINSIC)
    .fetch_all(pool)
    .await
    .map_err(|e| {
        DataError::Rpc(format!(
            "events_for_pairs(net={network_sid}, {} pairs): {e}",
            pairs.len()
        ))
    })?;

    let mut out: HashMap<(u64, u32), Vec<EventRef>> = HashMap::with_capacity(pairs.len());
    for (b, i, pallet, variant, data_scale) in rows {
        let fields = decode_event_fields(&pallet, &variant, &data_scale, ss58_prefix);
        out.entry((b.max(0) as u64, i.max(0) as u32))
            .or_default()
            .push(EventRef {
                module: pallet,
                name: variant,
                fields,
            });
    }
    Ok(out)
}
