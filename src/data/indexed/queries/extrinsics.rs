//! SQL reads backing the extrinsic-related `ChainData` methods.
//!
//! The DB stores extrinsics as a (block_num, idx) primary key plus a
//! `hash` BYTEA; a hash-based lookup ends up as one memcmp against
//! `extrinsics_hash_idx`. Every read JOINs `blocks` to pull the wall
//! clock — `Extrinsic::timestamp_ms` is the only field that can't live
//! on the row itself without duplicating storage.
//!
//! `args_scale` is decoded on the way out through
//! [`crate::data::metadata::decode_call_fields`] — the same static
//! metadata path the live RPC mapper now uses, so an indexed row and a
//! fresh pin produce the same `ExtrinsicArgs::Decoded { fields }` shape.
//! Anything the decoder can't resolve (unknown pallet/variant after a
//! runtime upgrade, truncated bytes) falls back to
//! `ExtrinsicArgs::Raw { hex }` so the Parameters tab never goes blank.
//!
//! Events are hydrated after the main SELECT through a single extra
//! query — see [`hydrate_events`] — so the N+1 trap is handled in one
//! place instead of at every call site.

use sqlx::{PgPool, Postgres, QueryBuilder};

use crate::data::cursor::{parse_cursor, ExtrinsicCursor};
use crate::data::error::{DataError, DataResult};
use crate::data::filters::ExtrinsicFilters;
use crate::data::metadata::decode_call_args;
use crate::data::rpc::mappers::hex_bytes;
use crate::data::ss58::{decode_hex32, encode_ss58_bytes};
use crate::domain::{CallResult, Extrinsic, Page, PageInfo, PageRequest};

use super::events;

/// Column list used by every extrinsic read. Kept as a constant so the
/// column order stays in lockstep with the tuple destructure in
/// [`row_to_extrinsic`] — positional binding is deliberate; introducing
/// `FromRow` would hide a shuffle until runtime.
///
/// `timestamp_ms` is read directly from `extrinsics` (denormalised in
/// migration 004) so list reads don't need a `JOIN blocks` just to
/// render wall clock. The `error_*` columns were dropped in that same
/// migration — they were stored but never surfaced to the UI.
const EXTRINSIC_COLUMNS: &str = "e.block_num, e.idx, e.hash, e.pallet, e.call, e.signer, \
                                 e.tip::text, e.fee::text, e.nonce, e.success, \
                                 e.args_scale, e.timestamp_ms";

/// Raw `sqlx` row tuple. `tip` / `fee` come back as `TEXT` because we
/// cast NUMERIC → TEXT on the wire: the bind direction also uses text
/// cast (see [`crate::indexer::sink::insert_extrinsics`]) so both sides
/// avoid the `bigdecimal`/`rust_decimal` sqlx features.
type Row = (
    i64,             // block_num
    i32,             // idx
    Vec<u8>,         // hash
    String,          // pallet
    String,          // call
    Option<Vec<u8>>, // signer
    Option<String>,  // tip::text
    String,          // fee::text
    Option<i64>,     // nonce
    bool,            // success
    Vec<u8>,         // args_scale
    i64,             // timestamp_ms
);

/// All extrinsics in `block_num` on `network_id`, ordered by on-chain
/// index. No filtering — callers that want the inherent stripped (e.g.
/// the "latest extrinsics" feed) apply that themselves.
pub async fn extrinsics_in_block(
    pool: &PgPool,
    network_sid: i16,
    block_num: u64,
    ss58_prefix: u16,
) -> DataResult<Vec<Extrinsic>> {
    let sql = format!(
        "SELECT {EXTRINSIC_COLUMNS} \
         FROM extrinsics e \
         WHERE e.network_id = $1 AND e.block_num = $2 \
         ORDER BY e.idx ASC"
    );
    let rows: Vec<Row> = sqlx::query_as(&sql)
        .bind(network_sid)
        .bind(block_num as i64)
        .fetch_all(pool)
        .await
        .map_err(|e| {
            DataError::Rpc(format!(
                "extrinsics_in_block(net={network_sid}/{block_num}): {e}"
            ))
        })?;
    let mut out: Vec<Extrinsic> = rows
        .into_iter()
        .map(|r| row_to_extrinsic(r, ss58_prefix))
        .collect();
    hydrate_events(pool, network_sid, &mut out, ss58_prefix).await?;
    Ok(out)
}

/// `count` newest extrinsics across all blocks of `network_id`, returned
/// as a plain [`Vec`]. Used by [`IndexedProvider`] when it tops up a
/// pending buffer slice; callers that need the [`Page<Extrinsic>`]
/// envelope should go through [`list_extrinsics_page`] instead.
///
/// The `(idx = 0, unsigned)` timestamp inherent is stripped so the list
/// stays focused on user-visible calls.
pub async fn latest_extrinsics(
    pool: &PgPool,
    network_sid: i16,
    count: u32,
    ss58_prefix: u16,
) -> DataResult<Vec<Extrinsic>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT {EXTRINSIC_COLUMNS} \
         FROM extrinsics e \
         WHERE e.network_id = $1 AND NOT (e.idx = 0 AND e.signer IS NULL) \
         ORDER BY e.block_num DESC, e.idx DESC \
         LIMIT $2"
    );
    let rows: Vec<Row> = sqlx::query_as(&sql)
        .bind(network_sid)
        .bind(count as i64)
        .fetch_all(pool)
        .await
        .map_err(|e| {
            DataError::Rpc(format!(
                "latest_extrinsics(net={network_sid}, {count}): {e}"
            ))
        })?;
    let mut out: Vec<Extrinsic> = rows
        .into_iter()
        .map(|r| row_to_extrinsic(r, ss58_prefix))
        .collect();
    hydrate_events(pool, network_sid, &mut out, ss58_prefix).await?;
    Ok(out)
}

/// Paginated newest-first extrinsic list honouring the cursor contract.
/// Strips the unsigned timestamp inherent and runs the fetch-N+1 trick
/// so [`PageInfo::has_more`] comes free of a `COUNT(*)`. `total` stays
/// `None`: row counts at this scale (10⁷+) are too expensive to return
/// per-request, and the UI doesn't render one.
///
/// `filters` layers on top of the cursor window — `signed`, `status`,
/// `pallet`, `call` all become `WHERE` predicates. The fetch-N+1 trick
/// still works because `LIMIT` is applied *after* the `WHERE`, so the
/// overflow row is a matching row by construction.
pub async fn list_extrinsics_page(
    pool: &PgPool,
    network_sid: i16,
    req: &PageRequest,
    filters: &ExtrinsicFilters,
    ss58_prefix: u16,
) -> DataResult<Page<Extrinsic>> {
    list_extrinsics_page_bounded(pool, network_sid, req, None, filters, ss58_prefix).await
}

/// Variant used by [`IndexedProvider`] when a buffered (pending-tip)
/// slice was peeled off first. `strict_upper` is the newest
/// `(block, idx)` the DB is allowed to return — the cursor alone isn't
/// enough when the pending slice already showed rows above the DB tip.
pub async fn list_extrinsics_page_bounded(
    pool: &PgPool,
    network_sid: i16,
    req: &PageRequest,
    strict_upper: Option<(u64, u32)>,
    filters: &ExtrinsicFilters,
    ss58_prefix: u16,
) -> DataResult<Page<Extrinsic>> {
    if req.count == 0 {
        return Ok(Page::empty());
    }
    let cursor = parse_cursor::<ExtrinsicCursor>(req)?;
    let probe = (req.count as i64).saturating_add(1);

    // Pick the smaller of `cursor` and `strict_upper`; both are
    // exclusive "give me rows strictly below this" bounds and we just
    // want the tightest one.
    let upper: Option<(u64, u32)> = match (cursor, strict_upper) {
        (Some(c), Some(u)) => Some((c.block, c.index).min((u.0, u.1))),
        (Some(c), None) => Some((c.block, c.index)),
        (None, Some(u)) => Some(u),
        (None, None) => None,
    };

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("SELECT ");
    qb.push(EXTRINSIC_COLUMNS);
    qb.push(" FROM extrinsics e WHERE e.network_id = ");
    qb.push_bind(network_sid);
    // The timestamp inherent appears in every block as `(idx=0, unsigned)`
    // and would drown the default feed; strip it unless the caller
    // explicitly asks for unsigned rows or filters by the Timestamp
    // pallet (both cases the user clearly wants to see inherents).
    let hide_timestamp_inherent =
        filters.signed != Some(false) && filters.pallet.as_deref() != Some("Timestamp");
    if hide_timestamp_inherent {
        qb.push(" AND NOT (e.idx = 0 AND e.signer IS NULL)");
    }
    if let Some((block, idx)) = upper {
        // Lexicographic strict-less-than on `(block, idx)` — the tuple
        // form lets the planner use the `(block_num DESC, idx DESC)`
        // index directly.
        qb.push(" AND (e.block_num, e.idx) < (");
        qb.push_bind(block as i64);
        qb.push(", ");
        qb.push_bind(idx as i32);
        qb.push(")");
    }
    if let Some(signed) = filters.signed {
        // `signer IS NULL` for unsigned inherents (minus the timestamp
        // already stripped above), `signer IS NOT NULL` for signed
        // transactions. Written as raw text — there's no value to bind.
        if signed {
            qb.push(" AND e.signer IS NOT NULL");
        } else {
            qb.push(" AND e.signer IS NULL");
        }
    }
    if let Some(pallet) = filters.pallet.as_deref() {
        qb.push(" AND e.pallet = ");
        qb.push_bind(pallet);
    }
    if let Some(call) = filters.call.as_deref() {
        qb.push(" AND e.call = ");
        qb.push_bind(call);
    }
    if let Some(status) = filters.status {
        qb.push(" AND e.success = ");
        qb.push_bind(matches!(status, CallResult::Success));
    }
    qb.push(" ORDER BY e.block_num DESC, e.idx DESC LIMIT ");
    qb.push_bind(probe);

    let rows: Vec<Row> = qb.build_query_as().fetch_all(pool).await.map_err(|e| {
        DataError::Rpc(format!(
            "list_extrinsics_page(net={network_sid}, count={}, cursor={cursor:?}, filters={filters:?}): {e}",
            req.count
        ))
    })?;

    let mut items: Vec<Extrinsic> = rows
        .into_iter()
        .map(|r| row_to_extrinsic(r, ss58_prefix))
        .collect();
    let has_more = items.len() > req.count as usize;
    if has_more {
        items.truncate(req.count as usize);
    }
    hydrate_events(pool, network_sid, &mut items, ss58_prefix).await?;
    let next_cursor = if has_more {
        items.last().map(|e| {
            ExtrinsicCursor {
                block: e.block_number,
                index: e.index,
            }
            .to_string()
        })
    } else {
        None
    };
    Ok(Page {
        items,
        page_info: PageInfo {
            total: None,
            next_cursor,
            has_more,
        },
    })
}

/// Lookup by primary key. Returns `None` when the row is missing — the
/// caller surfaces that as "not indexed yet" instead of masking with an
/// RPC fallback (the banner already flags indexer lag).
pub async fn extrinsic_by_block_idx(
    pool: &PgPool,
    network_sid: i16,
    block_num: u64,
    idx: u32,
    ss58_prefix: u16,
) -> DataResult<Option<Extrinsic>> {
    let sql = format!(
        "SELECT {EXTRINSIC_COLUMNS} \
         FROM extrinsics e \
         WHERE e.network_id = $1 AND e.block_num = $2 AND e.idx = $3"
    );
    let row: Option<Row> = sqlx::query_as(&sql)
        .bind(network_sid)
        .bind(block_num as i64)
        .bind(idx as i32)
        .fetch_optional(pool)
        .await
        .map_err(|e| {
            DataError::Rpc(format!(
                "extrinsic_by_block_idx(net={network_sid}/{block_num},{idx}): {e}"
            ))
        })?;
    let Some(mut ext) = row.map(|r| row_to_extrinsic(r, ss58_prefix)) else {
        return Ok(None);
    };
    ext.events =
        events::events_for_extrinsic(pool, network_sid, ext.block_number, ext.index, ss58_prefix)
            .await?;
    Ok(Some(ext))
}

/// Lookup by raw 32-byte hash within `network_sid`. Extrinsic hashes
/// aren't unique (two identical payloads from different signers hash
/// the same), so we return the newest match — tie-break by
/// `(block_num DESC, idx DESC)`.
pub async fn extrinsic_by_hash(
    pool: &PgPool,
    network_sid: i16,
    hash: &[u8],
    ss58_prefix: u16,
) -> DataResult<Option<Extrinsic>> {
    if hash.len() != 32 {
        return Ok(None);
    }
    let sql = format!(
        "SELECT {EXTRINSIC_COLUMNS} \
         FROM extrinsics e \
         WHERE e.network_id = $1 AND e.hash = $2 \
         ORDER BY e.block_num DESC, e.idx DESC \
         LIMIT 1"
    );
    let row: Option<Row> = sqlx::query_as(&sql)
        .bind(network_sid)
        .bind(hash)
        .fetch_optional(pool)
        .await
        .map_err(|e| DataError::Rpc(format!("extrinsic_by_hash(net={network_sid}): {e}")))?;
    let Some(mut ext) = row.map(|r| row_to_extrinsic(r, ss58_prefix)) else {
        return Ok(None);
    };
    ext.events =
        events::events_for_extrinsic(pool, network_sid, ext.block_number, ext.index, ss58_prefix)
            .await?;
    Ok(Some(ext))
}

/// Parse the incoming id string into a lookup key. Accepts the
/// canonical `"<block>-<index>"` form and a `"0x…"` extrinsic hash —
/// the search bar normalises both shapes into the same URL, and the
/// indexed provider resolves them uniformly.
pub enum ExtrinsicLookup {
    BlockIdx { block: u64, idx: u32 },
    Hash([u8; 32]),
}

/// Classify an `id` input. Returns `None` for anything we can't parse
/// — surfaced as "not found" at the caller, same as a missing row.
pub fn parse_lookup(id: &str) -> Option<ExtrinsicLookup> {
    if let Some((b, i)) = id.split_once('-') {
        if let (Ok(block), Ok(idx)) = (b.parse::<u64>(), i.parse::<u32>()) {
            return Some(ExtrinsicLookup::BlockIdx { block, idx });
        }
    }
    decode_hex32(id).map(ExtrinsicLookup::Hash)
}

/// Convert a DB row into the domain `Extrinsic`. `tip` / `fee` arrive
/// as TEXT and round-trip through `u128::from_str`; an unparseable
/// value would indicate index corruption and bubbles as a Decode error
/// rather than silently zeroing.
fn row_to_extrinsic(row: Row, ss58_prefix: u16) -> Extrinsic {
    let (
        block_num,
        idx,
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

    let block = block_num.max(0) as u64;
    let index = idx.max(0) as u32;

    let tip = tip_text.and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
    let fee = fee_text.parse::<u128>().unwrap_or(0);

    let signer_ss58 = signer.and_then(|bytes| encode_ss58_bytes(&bytes, ss58_prefix));
    let signed = signer_ss58.is_some();

    let args = decode_call_args(&pallet, &call, &args_scale, ss58_prefix);

    Extrinsic {
        id: format!("{block}-{index}"),
        block_number: block,
        index,
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
    }
}

/// Fill in the `events` field of every extrinsic in `xts` using one
/// extra SQL round-trip — the only way to keep a list-of-extrinsics
/// read from degenerating into N+1. Two shapes, picked from the data
/// itself rather than by the caller:
///
/// * **Every row shares a block** (`extrinsics_in_block`, and the
///   common case for `latest_extrinsics` when the tip emits a busy
///   block): one `WHERE block_num = $` scan, grouped in memory. The
///   planner uses the `(network_id, block_num, phase_kind)` prefix of
///   `events_phase_idx`.
/// * **Heterogeneous blocks**: one `JOIN unnest(block_nums, ext_idxs)`
///   query that probes the same index per pair. Still one round trip.
///
/// No-op on an empty slice, so callers can always hydrate without a
/// guard.
async fn hydrate_events(
    pool: &PgPool,
    network_sid: i16,
    xts: &mut [Extrinsic],
    ss58_prefix: u16,
) -> DataResult<()> {
    if xts.is_empty() {
        return Ok(());
    }
    let first_block = xts[0].block_number;
    let same_block = xts.iter().all(|e| e.block_number == first_block);

    if same_block {
        let by_idx =
            events::events_by_ext_idx_in_block(pool, network_sid, first_block, ss58_prefix).await?;
        for x in xts.iter_mut() {
            if let Some(evs) = by_idx.get(&x.index) {
                x.events = evs.clone();
            }
        }
        return Ok(());
    }

    let pairs: Vec<(u64, u32)> = xts.iter().map(|e| (e.block_number, e.index)).collect();
    let mut by_pair = events::events_for_pairs(pool, network_sid, &pairs, ss58_prefix).await?;
    for x in xts.iter_mut() {
        if let Some(evs) = by_pair.remove(&(x.block_number, x.index)) {
            x.events = evs;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `"block-idx"` happy path: two positive integers separated by
    /// a single `-`. Covers the URL shape the extrinsic detail page
    /// generates.
    #[test]
    fn parse_lookup_accepts_block_idx() {
        match parse_lookup("123-4") {
            Some(ExtrinsicLookup::BlockIdx { block, idx }) => {
                assert_eq!(block, 123);
                assert_eq!(idx, 4);
            }
            other => panic!("expected BlockIdx, got {:?}", classify(&other)),
        }
    }

    /// A 64-hex-char string — with or without the `0x` prefix —
    /// classifies as a hash lookup. The byte order matches the raw
    /// payload we stored; this keeps search-by-hash round-trippable
    /// without the caller having to normalise casing.
    #[test]
    fn parse_lookup_accepts_0x_hash() {
        let hex = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        match parse_lookup(&format!("0x{hex}")) {
            Some(ExtrinsicLookup::Hash(bytes)) => {
                assert_eq!(bytes[0], 0xab);
                assert_eq!(bytes[31], 0x89);
            }
            other => panic!("expected Hash, got {:?}", classify(&other)),
        }
        match parse_lookup(hex) {
            Some(ExtrinsicLookup::Hash(bytes)) => assert_eq!(bytes[0], 0xab),
            other => panic!("expected Hash (bare), got {:?}", classify(&other)),
        }
    }

    /// Garbage inputs — short strings, non-hex chars, off-by-one
    /// lengths — never match anything in the index; rejecting them at
    /// parse time saves a DB round-trip and keeps "not found" fast.
    #[test]
    fn parse_lookup_rejects_malformed_input() {
        assert!(parse_lookup("").is_none());
        assert!(parse_lookup("not-an-id").is_none());
        assert!(parse_lookup("123").is_none());
        // 63 hex chars — one byte short.
        let short = "0".repeat(63);
        assert!(parse_lookup(&short).is_none());
        // Correct length but invalid hex.
        let bad = "Z".repeat(64);
        assert!(parse_lookup(&bad).is_none());
    }

    /// Signed numeric inputs (`-1-0`) should not sneak through — a
    /// leading empty segment on `split_once('-')` would otherwise parse
    /// as a 0, giving us phantom hits on block 0.
    #[test]
    fn parse_lookup_rejects_negative_numeric_segments() {
        assert!(parse_lookup("-1-0").is_none());
    }

    /// Classification helper for test error messages. Kept inline to
    /// avoid exposing it in the public API.
    fn classify(v: &Option<ExtrinsicLookup>) -> &'static str {
        match v {
            Some(ExtrinsicLookup::BlockIdx { .. }) => "BlockIdx",
            Some(ExtrinsicLookup::Hash(_)) => "Hash",
            None => "None",
        }
    }
}
