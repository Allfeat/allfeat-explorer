//! SQL reads backing the transfer-related `ChainData` methods.
//!
//! The schema records every `Balances.Transfer` event as **two rows** in
//! `balance_movements` (one per account, opposite signs — see
//! `docs/indexing-plan.md` §1 + the `MovementKind` enum in
//! [`crate::indexer::projections::balances`]). To list one row per
//! transfer we filter on the *sender* side (`delta < 0`); the recipient
//! side is reachable through `counterparty`.
//!
//! Each row is then enriched with the extrinsic that emitted the event
//! (`events.phase_idx` → `extrinsics.idx`) and the block timestamp,
//! producing the `domain::Transfer` shape the UI already renders.

use sqlx::{PgPool, Postgres, QueryBuilder};
use subxt::utils::AccountId32;

use crate::data::cursor::{parse_cursor, TransferCursor};
use crate::data::error::{DataError, DataResult};
use crate::data::filters::TransferFilters;
use crate::data::metadata::decode_call_args;
use crate::data::rpc::mappers::hex_bytes;
use crate::data::ss58::encode_ss58_bytes;
use crate::domain::{CallResult, Extrinsic, Page, PageInfo, PageRequest, Transfer};

/// Parse an SS58 address that arrived as a filter value into the 32-byte
/// form the DB stores. A malformed address is a client error (the user
/// typed something invalid), so the response is `BadRequest` — a silent
/// "no matches" would hide the mistake.
fn parse_ss58_filter(value: &str, field: &str) -> DataResult<[u8; 32]> {
    value
        .parse::<AccountId32>()
        .map(|id| id.0)
        .map_err(|_| DataError::BadRequest(format!("invalid SS58 address in {field}: {value:?}")))
}

/// Column list shared by every transfer read. Kept in sync with the
/// tuple destructure in [`row_to_transfer`] — positional binding, not
/// `FromRow`, so a column shuffle here must be reflected there.
///
/// `timestamp_ms` is read from the joined `extrinsics` row
/// (denormalised in migration 004) so the transfer feed no longer
/// needs a `JOIN blocks` just for wall clock — one less 3M-row JOIN
/// per list page.
const TRANSFER_COLUMNS: &str =
    "m.block_num, m.event_idx, m.account, m.counterparty, m.delta::text, \
                                e.phase_idx, \
                                x.hash, x.pallet, x.call, x.signer, x.tip::text, x.fee::text, \
                                x.nonce, x.success, x.args_scale, \
                                x.timestamp_ms";

/// Raw row tuple from the JOIN. NUMERIC columns ride TEXT so the indexer
/// crate doesn't have to pull a decimal sqlx feature (mirrors the
/// extrinsics query).
type Row = (
    i64,             // block_num
    i32,             // event_idx
    Vec<u8>,         // account (sender, since we filter delta < 0)
    Option<Vec<u8>>, // counterparty (recipient)
    String,          // delta::text (signed magnitude — always negative on this slice)
    Option<i32>,     // phase_idx
    Vec<u8>,         // x.hash
    String,          // x.pallet
    String,          // x.call
    Option<Vec<u8>>, // x.signer
    Option<String>,  // x.tip::text
    String,          // x.fee::text
    Option<i64>,     // x.nonce
    bool,            // x.success
    Vec<u8>,         // x.args_scale
    i64,             // b.timestamp_ms
);

/// `count` newest transfers on `network_id`. Returns one row per
/// `Balances.Transfer` event (sender side); the recipient is carried
/// in `Transfer::to`. Used by [`IndexedProvider`] when topping up a
/// pending buffer slice; callers that want the [`Page<Transfer>`]
/// envelope should go through [`list_transfers_page`].
pub async fn latest_transfers(
    pool: &PgPool,
    network_id: &str,
    network_sid: i16,
    count: u32,
    ss58_prefix: u16,
) -> DataResult<Vec<Transfer>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT {TRANSFER_COLUMNS} \
         FROM balance_movements m \
         JOIN events e ON e.network_id = m.network_id AND e.block_num = m.block_num AND e.idx = m.event_idx \
         JOIN extrinsics x ON x.network_id = m.network_id AND x.block_num = m.block_num AND x.idx = e.phase_idx \
         WHERE m.network_id = $1 AND m.kind = 0 AND m.delta < 0 \
         ORDER BY m.block_num DESC, m.event_idx DESC \
         LIMIT $2"
    );
    let rows: Vec<Row> = sqlx::query_as(&sql)
        .bind(network_sid)
        .bind(count as i64)
        .fetch_all(pool)
        .await
        .map_err(|e| {
            DataError::Rpc(format!("latest_transfers(net={network_sid}, {count}): {e}"))
        })?;
    Ok(rows
        .into_iter()
        .filter_map(|row| row_to_transfer(row, network_id, ss58_prefix))
        .collect())
}

/// Paginated newest-first transfer list. Cursor is `(block, event_idx)`
/// (newest-first) and the fetch-N+1 trick lights up `has_more` without
/// a `COUNT(*)`. `total` stays `None`: transfer counts at full-chain
/// scale would need an `indexer_counters` table the plan defers.
///
/// `filters.from` / `filters.to` are exact SS58 matches. The sender side
/// of a transfer is `m.account` (we filter on the `delta < 0` slice), so
/// `from` binds the account column; `to` binds `counterparty`. Both
/// values hit `balance_movements_account_idx` when specified in
/// isolation; combined, the planner can pick whichever is more
/// selective.
pub async fn list_transfers_page(
    pool: &PgPool,
    network_id: &str,
    network_sid: i16,
    req: &PageRequest,
    filters: &TransferFilters,
    ss58_prefix: u16,
) -> DataResult<Page<Transfer>> {
    list_transfers_page_bounded(
        pool,
        network_id,
        network_sid,
        req,
        None,
        filters,
        ss58_prefix,
    )
    .await
}

/// Variant used by [`IndexedProvider`] when pending-tip rows were peeled
/// off first. `strict_upper` is the newest `(block, event_idx)` the DB
/// may return.
pub async fn list_transfers_page_bounded(
    pool: &PgPool,
    network_id: &str,
    network_sid: i16,
    req: &PageRequest,
    strict_upper: Option<(u64, u32)>,
    filters: &TransferFilters,
    ss58_prefix: u16,
) -> DataResult<Page<Transfer>> {
    if req.count == 0 {
        return Ok(Page::empty());
    }
    let cursor = parse_cursor::<TransferCursor>(req)?;
    let probe = (req.count as i64).saturating_add(1);

    let upper: Option<(u64, u32)> = match (cursor, strict_upper) {
        (Some(c), Some(u)) => Some((c.block, c.event_idx).min((u.0, u.1))),
        (Some(c), None) => Some((c.block, c.event_idx)),
        (None, Some(u)) => Some(u),
        (None, None) => None,
    };

    // Parse SS58 filter values up-front — a BadRequest here is a 400
    // at the HTTP layer, not a silent empty page.
    let from_bytes: Option<[u8; 32]> = match filters.from.as_deref() {
        Some(s) => Some(parse_ss58_filter(s, "from")?),
        None => None,
    };
    let to_bytes: Option<[u8; 32]> = match filters.to.as_deref() {
        Some(s) => Some(parse_ss58_filter(s, "to")?),
        None => None,
    };

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("SELECT ");
    qb.push(TRANSFER_COLUMNS);
    qb.push(
        " FROM balance_movements m \
         JOIN events e ON e.network_id = m.network_id AND e.block_num = m.block_num AND e.idx = m.event_idx \
         JOIN extrinsics x ON x.network_id = m.network_id AND x.block_num = m.block_num AND x.idx = e.phase_idx \
         WHERE m.network_id = ",
    );
    qb.push_bind(network_sid);
    qb.push(" AND m.kind = 0 AND m.delta < 0");
    if let Some((block, event_idx)) = upper {
        qb.push(" AND (m.block_num, m.event_idx) < (");
        qb.push_bind(block as i64);
        qb.push(", ");
        qb.push_bind(event_idx as i32);
        qb.push(")");
    }
    // BYTEA bindings need an owned Vec<u8> (sqlx accepts &[u8] only by
    // reference, and the QueryBuilder bind takes by value). Binding the
    // 32-byte array directly would need a lifetime helper; a Vec keeps
    // the ergonomics straightforward at the cost of two tiny allocs.
    if let Some(bytes) = from_bytes {
        qb.push(" AND m.account = ");
        qb.push_bind(bytes.to_vec());
    }
    if let Some(bytes) = to_bytes {
        qb.push(" AND m.counterparty = ");
        qb.push_bind(bytes.to_vec());
    }
    qb.push(" ORDER BY m.block_num DESC, m.event_idx DESC LIMIT ");
    qb.push_bind(probe);

    let rows: Vec<Row> = qb.build_query_as().fetch_all(pool).await.map_err(|e| {
        DataError::Rpc(format!(
            "list_transfers_page(net={network_sid}, count={}, cursor={cursor:?}, filters={filters:?}): {e}",
            req.count
        ))
    })?;

    // `row_to_transfer` can drop rows with bad payloads (orphan phases,
    // malformed bytes). Keep each (block, event_idx) alongside the
    // converted transfer so we can rebuild the cursor without needing
    // event_idx on the domain type. Doing the filter before the N+1
    // truncation means a single corrupt row could silently understate
    // `has_more`, but counting filtered rows against the probe would
    // paginate phantom items; row-level corruption is rare enough that
    // consistency wins here.
    let mut probed: Vec<(Transfer, u64, u32)> = rows
        .into_iter()
        .filter_map(|row| {
            let block = row.0.max(0) as u64;
            let event_idx = row.1.max(0) as u32;
            row_to_transfer(row, network_id, ss58_prefix).map(|t| (t, block, event_idx))
        })
        .collect();
    let has_more = probed.len() > req.count as usize;
    if has_more {
        probed.truncate(req.count as usize);
    }
    let next_cursor = if has_more {
        probed.last().map(|(_, block, event_idx)| {
            TransferCursor {
                block: *block,
                event_idx: *event_idx,
            }
            .to_string()
        })
    } else {
        None
    };
    let items: Vec<Transfer> = probed.into_iter().map(|(t, _, _)| t).collect();
    Ok(Page {
        items,
        page_info: PageInfo {
            total: None,
            next_cursor,
            has_more,
        },
    })
}

/// History of transfers on `network_id` involving `account` (either
/// side). Same enrich path as [`latest_transfers`] but joined on
/// `m.account = $2` to take advantage of `balance_movements_account_idx`.
pub async fn account_transfer_history(
    pool: &PgPool,
    network_id: &str,
    network_sid: i16,
    account: &[u8; 32],
    count: u32,
    ss58_prefix: u16,
) -> DataResult<Vec<Transfer>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT {TRANSFER_COLUMNS} \
         FROM balance_movements m \
         JOIN events e ON e.network_id = m.network_id AND e.block_num = m.block_num AND e.idx = m.event_idx \
         JOIN extrinsics x ON x.network_id = m.network_id AND x.block_num = m.block_num AND x.idx = e.phase_idx \
         WHERE m.network_id = $1 AND m.kind = 0 AND m.account = $2 \
         ORDER BY m.block_num DESC, m.event_idx DESC \
         LIMIT $3"
    );
    let rows: Vec<Row> = sqlx::query_as(&sql)
        .bind(network_sid)
        .bind(&account[..])
        .bind(count as i64)
        .fetch_all(pool)
        .await
        .map_err(|e| DataError::Rpc(format!("account_transfer_history(net={network_sid}): {e}")))?;
    Ok(rows
        .into_iter()
        .filter_map(|row| row_to_transfer(row, network_id, ss58_prefix))
        .collect())
}

/// Decode a row into the `domain::Transfer` shape the UI consumes.
/// Returns `None` when an invariant fails (missing `phase_idx`, garbled
/// numeric, missing counterparty) — the caller filters those out rather
/// than failing the whole feed; a corrupted row shouldn't blank out an
/// otherwise-healthy list.
fn row_to_transfer(row: Row, network_id: &str, ss58_prefix: u16) -> Option<Transfer> {
    let (
        block_num,
        event_idx,
        account_bytes,
        counterparty_bytes,
        delta_text,
        phase_idx,
        hash,
        pallet,
        call,
        signer,
        tip_text,
        fee_text,
        nonce,
        success,
        args_scale,
        timestamp_ms,
    ) = row;

    // The transfer feed needs an extrinsic id to render — orphan rows
    // (delta with no enclosing ApplyExtrinsic phase, e.g. a future
    // runtime that mints during initialization) skip the list rather
    // than landing without a clickable target.
    let phase_idx = phase_idx?;
    let block = block_num.max(0) as u64;
    let xt_index = phase_idx.max(0) as u32;

    let account_str = encode_ss58_bytes(&account_bytes, ss58_prefix)?;
    let counterparty_str = encode_ss58_bytes(counterparty_bytes.as_deref()?, ss58_prefix)?;

    // Sign on the row tells us which side this account is on. Negative
    // delta ⇒ this account sent the transfer. Positive ⇒ received it.
    // We always present the transfer in `(from → to, amount)` shape so
    // the UI doesn't have to recompute the direction.
    let delta_i128: i128 = delta_text.parse().ok()?;
    let amount = delta_i128.unsigned_abs();
    let (from, to) = if delta_i128 < 0 {
        (account_str, counterparty_str)
    } else {
        (counterparty_str, account_str)
    };

    let signer_ss58 = signer.and_then(|bytes| encode_ss58_bytes(&bytes, ss58_prefix));
    let signed = signer_ss58.is_some();
    let tip = tip_text.and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
    let fee = fee_text.parse::<u128>().unwrap_or(0);

    let args = decode_call_args(network_id, &pallet, &call, &args_scale, ss58_prefix);

    let extrinsic = Extrinsic {
        id: format!("{block}-{xt_index}"),
        block_number: block,
        index: xt_index,
        hash: hex_bytes(&hash),
        module: pallet,
        call,
        signed,
        signer: signer_ss58,
        args,
        result: if success {
            CallResult::Success
        } else {
            CallResult::Failed
        },
        nonce: nonce.map(|n| n as u32),
        tip,
        fee,
        timestamp_ms,
        events: Vec::new(),
    };

    let _ = event_idx; // kept in the row for symmetry with the indexed PK; not surfaced on the domain type
    Some(Transfer {
        extrinsic,
        from,
        to,
        amount,
    })
}
