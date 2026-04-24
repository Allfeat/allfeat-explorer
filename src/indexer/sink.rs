//! The only module that issues `INSERT` / `UPDATE` statements against
//! Postgres. Projections stay pure, the sink owns every SQL write.
//!
//! Each function takes a `&mut sqlx::Transaction` so the caller can
//! commit several rows (block header, extrinsics, events, …) in one
//! atomic unit, and a `network_sid` (SMALLINT from the `networks`
//! lookup) because one deployment indexes several chains into the
//! same DB — every row must carry its origin.
//!
//! Interning: `network_id TEXT` is resolved to `SMALLINT` at the
//! worker boundary via [`super::lookups::NetworkLookup`] and
//! `blocks.author` is resolved to `authors.id` via
//! [`super::lookups::AuthorLookup`]. Both lookups live outside the
//! per-block transaction on purpose — an author row is longer-lived
//! than any single block insert. The sink binds the already-resolved
//! ids directly.
//!
//! Idempotency: every write is `ON CONFLICT DO NOTHING` (or a guarded
//! UPSERT). Rerunning a block is a no-op — the live worker and the
//! backfill workers race safely on overlapping ranges, and a process
//! that dies mid-block restarts without leaving half-blocks in the DB
//! (the tx rolls back if `commit()` never fires).

use sqlx::{PgPool, Postgres, QueryBuilder, Transaction};

use crate::data::error::{DataError, DataResult};

use super::projections::ats::AtsOps;
use super::projections::balances::BalanceMovementRow;
use super::projections::blocks::BlockRow;
use super::projections::events::EventRow;
use super::projections::extrinsics::ExtrinsicRow;
use crate::data::rpc::mappers::accounts::AccountSnapshot;

/// Cursor stream name for the live worker. Kept as a constant so the
/// endpoint cache, the migrations, and any reset/admin tooling read
/// from the same label. Scoped per network via the `(network_id,
/// stream)` composite PK on `indexer_cursor`.
pub const LIVE_CURSOR: &str = "live";

/// Conservative upper bound on bind parameters per multi-row INSERT.
/// Postgres's wire-protocol limit is 65535 (an `i16` range); we stay
/// well under it so the `MAX / col_count` chunk size always leaves
/// headroom for a few extra binds the callers might add later. Every
/// batch insert below chunks `rows.chunks(MAX_BINDS_PER_QUERY / cols)`.
const MAX_BINDS_PER_QUERY: usize = 65000;

/// Insert one block row. `ON CONFLICT (network_id, num) DO NOTHING`
/// handles the backfill-vs-live race and the restart-with-stale-cursor
/// replay without any bespoke upsert logic.
///
/// Returns `true` when a brand-new row landed, `false` on conflict. The
/// live + backfill workers use that flag to short-circuit the per-block
/// `System::Account` fan-out that feeds [`apply_account_snapshots`]: a
/// replay of a block we already indexed re-derives the same snapshots,
/// so skipping the RPC cost is purely an optimisation, not a
/// correctness requirement. Every sink call in this module is
/// idempotent — `ON CONFLICT DO NOTHING` or the `last_activity_block`
/// guard in the snapshot UPSERT — so they're safe to re-run.
pub async fn insert_block(
    tx: &mut Transaction<'_, Postgres>,
    network_sid: i16,
    row: &BlockRow,
    author_id: Option<i32>,
) -> DataResult<bool> {
    let res = sqlx::query(
        "INSERT INTO blocks (
             network_id, num, hash, parent_hash, state_root, extrinsics_root,
             author_id, timestamp_ms, spec_version, extrinsic_count, event_count,
             ref_time, proof_size, ref_time_pct, size_bytes
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
         ON CONFLICT (network_id, num) DO NOTHING",
    )
    .bind(network_sid)
    .bind(row.num as i64)
    .bind(&row.hash[..])
    .bind(&row.parent_hash[..])
    .bind(&row.state_root[..])
    .bind(&row.extrinsics_root[..])
    .bind(author_id)
    .bind(row.timestamp_ms)
    .bind(row.spec_version as i32)
    .bind(row.extrinsic_count as i32)
    .bind(row.event_count as i32)
    .bind(row.ref_time as i64)
    .bind(row.proof_size as i64)
    .bind(row.ref_time_pct as i16)
    .bind(row.size_bytes as i32)
    .execute(&mut **tx)
    .await
    .map_err(|e| DataError::Rpc(format!("insert_block(net={network_sid}/{}): {e}", row.num)))?;
    Ok(res.rows_affected() > 0)
}

/// Insert every extrinsic for one block in a single multi-row INSERT.
/// `ON CONFLICT (network_id, block_num, idx) DO NOTHING` keeps
/// restart-replay races no-ops.
///
/// `tip` and `fee` use `NUMERIC(39)` in the schema to hold the full
/// `u128` range. sqlx doesn't bind `u128` natively without the
/// `bigdecimal` feature, so we stringify and cast server-side with
/// `::NUMERIC` next to each bind — a planck literal under 39 digits
/// casts losslessly, and keeping the wire format as TEXT avoids
/// pulling a new decimal dependency into the indexer crate.
///
/// Chunked at `MAX_BINDS_PER_QUERY / 14` rows per statement so the
/// Postgres 65535-parameter cap can't bite on pathological blocks
/// (normal blocks are far under the threshold; the guard is defensive).
pub async fn insert_extrinsics(
    tx: &mut Transaction<'_, Postgres>,
    network_sid: i16,
    block_timestamp_ms: i64,
    rows: &[ExtrinsicRow],
) -> DataResult<()> {
    if rows.is_empty() {
        return Ok(());
    }

    // 13 columns now (error_module/error_name dropped in schema 004,
    // timestamp_ms added for JOIN elimination on list reads).
    const COLS: usize = 13;
    const CHUNK: usize = MAX_BINDS_PER_QUERY / COLS;

    for chunk in rows.chunks(CHUNK) {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO extrinsics (\
                 network_id, block_num, idx, hash, pallet, call, signer, \
                 tip, fee, nonce, success, args_scale, timestamp_ms\
             ) ",
        );
        qb.push_values(chunk.iter(), |mut b, row| {
            let signer_bytes: Option<Vec<u8>> = row.signer.map(|s| s.to_vec());
            let tip_text: Option<String> = row.tip.map(|v| v.to_string());
            let fee_text: String = row.fee.to_string();
            let nonce_signed: Option<i64> = row.nonce.map(|n| n as i64);

            b.push_bind(network_sid)
                .push_bind(row.block_num as i64)
                .push_bind(row.idx as i32)
                .push_bind(row.hash.to_vec())
                .push_bind(row.pallet.clone())
                .push_bind(row.call.clone())
                .push_bind(signer_bytes)
                .push_bind(tip_text)
                .push_unseparated("::NUMERIC")
                .push_bind(fee_text)
                .push_unseparated("::NUMERIC")
                .push_bind(nonce_signed)
                .push_bind(row.success)
                .push_bind(row.args_scale.clone())
                .push_bind(block_timestamp_ms);
        });
        qb.push(" ON CONFLICT (network_id, block_num, idx) DO NOTHING");
        qb.build().execute(&mut **tx).await.map_err(|e| {
            DataError::Rpc(format!(
                "insert_extrinsics(net={network_sid}, {} rows): {e}",
                chunk.len()
            ))
        })?;
    }
    Ok(())
}

/// Insert every event for one block in a single multi-row INSERT.
/// `(network_id, block_num, idx)` PK plus `ON CONFLICT DO NOTHING`
/// keeps the live ↔ backfill replay race no-ops, same contract as
/// [`insert_extrinsics`]. See that function for the chunking rationale.
pub async fn insert_events(
    tx: &mut Transaction<'_, Postgres>,
    network_sid: i16,
    block_timestamp_ms: i64,
    rows: &[EventRow],
) -> DataResult<()> {
    if rows.is_empty() {
        return Ok(());
    }

    // 9 columns now (timestamp_ms added for JOIN elimination on list reads).
    const COLS: usize = 9;
    const CHUNK: usize = MAX_BINDS_PER_QUERY / COLS;

    for chunk in rows.chunks(CHUNK) {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO events (\
                 network_id, block_num, idx, phase_kind, phase_idx, \
                 pallet, variant, data_scale, timestamp_ms\
             ) ",
        );
        qb.push_values(chunk.iter(), |mut b, row| {
            let phase_idx_signed: Option<i32> = row.phase_idx.map(|n| n as i32);
            b.push_bind(network_sid)
                .push_bind(row.block_num as i64)
                .push_bind(row.idx as i32)
                .push_bind(row.phase_kind)
                .push_bind(phase_idx_signed)
                .push_bind(row.pallet.clone())
                .push_bind(row.variant.clone())
                .push_bind(row.data_scale.clone())
                .push_bind(block_timestamp_ms);
        });
        qb.push(" ON CONFLICT (network_id, block_num, idx) DO NOTHING");
        qb.build().execute(&mut **tx).await.map_err(|e| {
            DataError::Rpc(format!(
                "insert_events(net={network_sid}, {} rows): {e}",
                chunk.len()
            ))
        })?;
    }
    Ok(())
}

/// Insert every balance movement for one block in a single multi-row
/// INSERT.
///
/// `delta` is bound as TEXT and cast to `NUMERIC` server-side — same
/// trick as `extrinsics.fee` so the indexer crate doesn't have to pull
/// `bigdecimal`/`rust_decimal`. `i128::to_string()` always round-trips
/// through `NUMERIC(39)`'s signed range without loss. See
/// [`insert_extrinsics`] for the chunking rationale.
pub async fn insert_balance_movements(
    tx: &mut Transaction<'_, Postgres>,
    network_sid: i16,
    rows: &[BalanceMovementRow],
) -> DataResult<()> {
    if rows.is_empty() {
        return Ok(());
    }

    const COLS: usize = 7;
    const CHUNK: usize = MAX_BINDS_PER_QUERY / COLS;

    for chunk in rows.chunks(CHUNK) {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO balance_movements (\
                 network_id, block_num, event_idx, account, kind, delta, counterparty\
             ) ",
        );
        qb.push_values(chunk.iter(), |mut b, row| {
            let counterparty: Option<Vec<u8>> = row.counterparty.map(|c| c.to_vec());
            let delta_text = row.delta.to_string();
            b.push_bind(network_sid)
                .push_bind(row.block_num as i64)
                .push_bind(row.event_idx as i32)
                .push_bind(row.account.to_vec())
                .push_bind(row.kind.as_i16())
                .push_bind(delta_text)
                .push_unseparated("::NUMERIC")
                .push_bind(counterparty);
        });
        qb.push(" ON CONFLICT (network_id, block_num, event_idx, account) DO NOTHING");
        qb.build().execute(&mut **tx).await.map_err(|e| {
            DataError::Rpc(format!(
                "insert_balance_movements(net={network_sid}, {} rows): {e}",
                chunk.len()
            ))
        })?;
    }
    Ok(())
}

/// UPSERT one block's per-account snapshots into `account_balances`.
///
/// Each snapshot is an *absolute* `System::Account` reading at
/// `block_num`, not a delta. Replay is a no-op: writing the same value
/// on top of itself leaves the row unchanged. Out-of-order writes are
/// safe too — the CASE clause below only overwrites free/reserved/frozen
/// when `EXCLUDED.last_activity_block >= account_balances.last_activity_block`,
/// so a backfill worker fetching an older block doesn't regress a row
/// the live worker has already advanced to a newer height.
///
/// `first_seen_block` is pinned to the earliest block the account was
/// snapshot-touched via `LEAST`, which matters when a backfill worker
/// lands earlier history after the live path has already recorded a
/// later first-touch. `nonce` uses `GREATEST` because reaped accounts
/// (where the snapshot carries `nonce = 0`) must not regress a
/// previously-observed counter — the runtime guards against practical
/// nonce reuse but the DB's copy should stay monotonic regardless.
///
/// Scoped per network via the `(network_id, account)` composite PK —
/// the same AccountId32 on two different chains holds independent
/// balances.
pub async fn apply_account_snapshots(
    tx: &mut Transaction<'_, Postgres>,
    network_sid: i16,
    block_num: u64,
    block_timestamp_ms: i64,
    snapshots: &[([u8; 32], AccountSnapshot)],
) -> DataResult<()> {
    if snapshots.is_empty() {
        return Ok(());
    }

    // 10 binds/row: network_sid, account, free, reserved, frozen,
    // nonce, first_seen_block, last_activity_block, first_seen_ms,
    // last_activity_ms. The block number + timestamp are bound twice
    // per row (first_seen = last_activity on the first write for an
    // account; the UPSERT guards `LEAST`/`GREATEST` keep them honest
    // on subsequent writes).
    const COLS: usize = 10;
    const CHUNK: usize = MAX_BINDS_PER_QUERY / COLS;

    let block_num_i64 = block_num as i64;

    for chunk in snapshots.chunks(CHUNK) {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO account_balances (\
                 network_id, account, free, reserved, frozen, nonce, \
                 first_seen_block, last_activity_block, \
                 first_seen_ms, last_activity_ms\
             ) ",
        );
        qb.push_values(chunk.iter(), |mut b, (account, snap)| {
            let free_text = snap.free.to_string();
            let reserved_text = snap.reserved.to_string();
            let frozen_text = snap.frozen.to_string();
            let nonce: i64 = snap.nonce as i64;

            b.push_bind(network_sid)
                .push_bind(account.to_vec())
                .push_bind(free_text)
                .push_unseparated("::NUMERIC")
                .push_bind(reserved_text)
                .push_unseparated("::NUMERIC")
                .push_bind(frozen_text)
                .push_unseparated("::NUMERIC")
                .push_bind(nonce)
                .push_bind(block_num_i64)
                .push_bind(block_num_i64)
                .push_bind(block_timestamp_ms)
                .push_bind(block_timestamp_ms);
        });
        qb.push(
            " ON CONFLICT (network_id, account) DO UPDATE SET \
                 free = CASE \
                     WHEN EXCLUDED.last_activity_block >= account_balances.last_activity_block \
                     THEN EXCLUDED.free \
                     ELSE account_balances.free \
                 END, \
                 reserved = CASE \
                     WHEN EXCLUDED.last_activity_block >= account_balances.last_activity_block \
                     THEN EXCLUDED.reserved \
                     ELSE account_balances.reserved \
                 END, \
                 frozen = CASE \
                     WHEN EXCLUDED.last_activity_block >= account_balances.last_activity_block \
                     THEN EXCLUDED.frozen \
                     ELSE account_balances.frozen \
                 END, \
                 nonce = GREATEST(account_balances.nonce, EXCLUDED.nonce), \
                 first_seen_block = LEAST( \
                     account_balances.first_seen_block, \
                     EXCLUDED.first_seen_block \
                 ), \
                 last_activity_block = GREATEST( \
                     account_balances.last_activity_block, \
                     EXCLUDED.last_activity_block \
                 ), \
                 first_seen_ms = LEAST( \
                     account_balances.first_seen_ms, \
                     EXCLUDED.first_seen_ms \
                 ), \
                 last_activity_ms = GREATEST( \
                     account_balances.last_activity_ms, \
                     EXCLUDED.last_activity_ms \
                 )",
        );
        qb.build().execute(&mut **tx).await.map_err(|e| {
            DataError::Rpc(format!(
                "apply_account_snapshots(net={network_sid}/block {block_num}, {} rows): {e}",
                chunk.len()
            ))
        })?;
    }
    Ok(())
}

/// One-shot reconciliation write: overwrite free/reserved/frozen for
/// every `account_balances` row named in `snapshots`, without touching
/// `first_seen_block` / `last_activity_block`.
///
/// The distinction from [`apply_account_snapshots`] matters: the
/// per-block UPSERT advances `last_activity_block` because a block that
/// touches an account *is* activity. Reconciliation is a maintenance
/// sweep — it corrects drift in the stored totals without lying that
/// the account had any on-chain activity at the head block. Nonce is
/// still clamped with `GREATEST` so a reaped account whose current
/// `System::Account.nonce = 0` doesn't regress a previously-seen
/// counter.
///
/// Returns the number of rows actually updated. Absent accounts (bytes
/// not present in `account_balances` — unreachable under normal use
/// since the caller feeds back the rows it just read) are silently
/// dropped; reconciliation never invents new rows. An explicit re-seed
/// path (iterate `System::Account` + INSERT) is a separate tool.
pub async fn reconcile_account_balances(
    tx: &mut Transaction<'_, Postgres>,
    network_sid: i16,
    snapshots: &[([u8; 32], AccountSnapshot)],
) -> DataResult<u64> {
    if snapshots.is_empty() {
        return Ok(0);
    }

    // 5 binds per row (account, free, reserved, frozen, nonce). `network_sid`
    // is bound once per statement in the WHERE clause. Same ceiling as the
    // per-block UPSERT above — we stay comfortably under Postgres's 65535
    // parameter limit.
    const COLS: usize = 5;
    const CHUNK: usize = MAX_BINDS_PER_QUERY / COLS;

    let mut updated: u64 = 0;
    for chunk in snapshots.chunks(CHUNK) {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "UPDATE account_balances AS ab SET \
                 free = v.free, \
                 reserved = v.reserved, \
                 frozen = v.frozen, \
                 nonce = GREATEST(ab.nonce, v.nonce) \
             FROM (",
        );
        qb.push_values(chunk.iter(), |mut b, (account, snap)| {
            b.push_bind(account.to_vec())
                .push_bind(snap.free.to_string())
                .push_unseparated("::NUMERIC")
                .push_bind(snap.reserved.to_string())
                .push_unseparated("::NUMERIC")
                .push_bind(snap.frozen.to_string())
                .push_unseparated("::NUMERIC")
                .push_bind(snap.nonce as i64);
        });
        qb.push(") AS v(account, free, reserved, frozen, nonce) WHERE ab.network_id = ");
        qb.push_bind(network_sid);
        qb.push(" AND ab.account = v.account");

        let res = qb.build().execute(&mut **tx).await.map_err(|e| {
            DataError::Rpc(format!(
                "reconcile_account_balances(net={network_sid}, {} rows): {e}",
                chunk.len()
            ))
        })?;
        updated += res.rows_affected();
    }
    Ok(updated)
}

/// Apply one block's aggregated ATS side-effects. Composition of
/// three independently-idempotent writes, in an order that matches the
/// pallet's own semantics:
///
/// 1. **Registry inserts** (creator events) — `ON CONFLICT (network_id,
///    id) DO NOTHING`. Replaying a create event is a no-op; the owner /
///    created_block never change after creation.
/// 2. **Version inserts** — `ON CONFLICT (network_id, ats_id, version)
///    DO NOTHING` so replays are safe. If the parent registry row
///    doesn't exist yet (version event indexed before its creator
///    landed) the FK constraint fails and we bubble the error.
/// 3. **Version count bumps** — `GREATEST(current, new)` so the count
///    only moves forward. A replayed or out-of-order bump is a no-op.
/// 4. **Revocations** — DELETEs cascade to `ats_versions` via the FK.
///    A revoke for an unknown id is a no-op (DELETE matches zero rows).
pub async fn apply_ats(
    tx: &mut Transaction<'_, Postgres>,
    network_sid: i16,
    ops: &AtsOps,
) -> DataResult<()> {
    if !ops.registry_inserts.is_empty() {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO ats_registry (\
                 network_id, id, owner, created_block, created_ext_idx, version_count\
             ) ",
        );
        qb.push_values(ops.registry_inserts.iter(), |mut b, row| {
            b.push_bind(network_sid)
                .push_bind(row.id as i64)
                .push_bind(row.owner.to_vec())
                .push_bind(row.created_block as i64)
                .push_bind(row.created_ext_idx as i32)
                .push_bind(row.version_count as i32);
        });
        qb.push(" ON CONFLICT (network_id, id) DO NOTHING");
        qb.build().execute(&mut **tx).await.map_err(|e| {
            DataError::Rpc(format!(
                "insert_ats_registry(net={network_sid}, {} rows): {e}",
                ops.registry_inserts.len()
            ))
        })?;
    }

    if !ops.version_inserts.is_empty() {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO ats_versions (\
                 network_id, ats_id, version, block_num, ext_idx, commitment, protocol_version\
             ) ",
        );
        qb.push_values(ops.version_inserts.iter(), |mut b, row| {
            b.push_bind(network_sid)
                .push_bind(row.ats_id as i64)
                .push_bind(row.version as i32)
                .push_bind(row.block_num as i64)
                .push_bind(row.ext_idx as i32)
                .push_bind(row.commitment.to_vec())
                .push_bind(row.protocol_version as i16);
        });
        qb.push(" ON CONFLICT (network_id, ats_id, version) DO NOTHING");
        qb.build().execute(&mut **tx).await.map_err(|e| {
            DataError::Rpc(format!(
                "insert_ats_version(net={network_sid}, {} rows): {e}",
                ops.version_inserts.len()
            ))
        })?;
    }

    for bump in &ops.version_count_bumps {
        sqlx::query(
            "UPDATE ats_registry
                SET version_count = GREATEST(version_count, $3)
              WHERE network_id = $1 AND id = $2",
        )
        .bind(network_sid)
        .bind(bump.ats_id as i64)
        .bind(bump.version_count as i32)
        .execute(&mut **tx)
        .await
        .map_err(|e| {
            DataError::Rpc(format!(
                "bump_version_count(net={network_sid}/{}, {}): {e}",
                bump.ats_id, bump.version_count
            ))
        })?;
    }

    for id in &ops.revocations {
        sqlx::query("DELETE FROM ats_registry WHERE network_id = $1 AND id = $2")
            .bind(network_sid)
            .bind(*id as i64)
            .execute(&mut **tx)
            .await
            .map_err(|e| DataError::Rpc(format!("revoke_ats(net={network_sid}/{id}): {e}")))?;
    }

    Ok(())
}

/// Advance the cursor for `(network_id, stream)` to `last`. Strictly
/// monotonic: a regressive update (lower `last_indexed`) is ignored,
/// not applied. That keeps the cursor honest even when a process
/// replays an older block through `ON CONFLICT DO NOTHING`.
pub async fn bump_cursor(
    tx: &mut Transaction<'_, Postgres>,
    network_sid: i16,
    stream: &str,
    last: u64,
) -> DataResult<()> {
    sqlx::query(
        "INSERT INTO indexer_cursor (network_id, stream, last_indexed, updated_at)
         VALUES ($1, $2, $3, now())
         ON CONFLICT (network_id, stream) DO UPDATE
             SET last_indexed = EXCLUDED.last_indexed,
                 updated_at   = now()
             WHERE indexer_cursor.last_indexed < EXCLUDED.last_indexed",
    )
    .bind(network_sid)
    .bind(stream)
    .bind(last as i64)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        DataError::Rpc(format!(
            "bump_cursor(net={network_sid}/{stream} → {last}): {e}"
        ))
    })?;
    Ok(())
}

/// Read the current cursor position for `(network_sid, stream)`.
/// `None` means the row doesn't exist yet (fresh DB or network never
/// indexed).
pub async fn load_cursor(pool: &PgPool, network_sid: i16, stream: &str) -> DataResult<Option<u64>> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT last_indexed FROM indexer_cursor WHERE network_id = $1 AND stream = $2",
    )
    .bind(network_sid)
    .bind(stream)
    .fetch_optional(pool)
    .await
    .map_err(|e| DataError::Rpc(format!("load_cursor(net={network_sid}/{stream}): {e}")))?;
    Ok(row.map(|(v,)| v as u64))
}

/// Seconds since the cursor row was last updated. Used by the status
/// endpoint to flag `Offline` when no write has landed for a while —
/// the cursor bumps on every indexed block, so a stale `updated_at`
/// means the live worker is stuck (or the process is gone).
///
/// Returns `None` if the cursor row doesn't exist yet.
pub async fn cursor_age_seconds(
    pool: &PgPool,
    network_sid: i16,
    stream: &str,
) -> DataResult<Option<i64>> {
    let row: Option<(f64,)> = sqlx::query_as(
        "SELECT EXTRACT(EPOCH FROM (now() - updated_at))::double precision
           FROM indexer_cursor
          WHERE network_id = $1 AND stream = $2",
    )
    .bind(network_sid)
    .bind(stream)
    .fetch_optional(pool)
    .await
    .map_err(|e| DataError::Rpc(format!("cursor_age(net={network_sid}/{stream}): {e}")))?;
    Ok(row.map(|(v,)| v as i64))
}
