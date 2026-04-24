//! SQL reads backing the block-related `ChainData` methods.
//!
//! Keeps every `SELECT` in one place so upgrades to the schema (new
//! columns, partitioning) touch exactly one file. Returns `domain::Block`
//! values already in the shape the UI expects; the row struct stays
//! private on purpose — no other module should depend on the exact
//! column layout.

use sqlx::{PgPool, Postgres, QueryBuilder};

use crate::data::cursor::{parse_cursor, BlockCursor};
use crate::data::error::{DataError, DataResult};
use crate::data::filters::BlockFilters;
use crate::data::rpc::mappers::hex_bytes;
use crate::data::ss58::{encode_ss58_bytes, short_label};
use crate::domain::{Block, Page, PageInfo, PageRequest};

/// Column list used by every block read below. Keep in sync with the
/// tuple destructuring in `row_to_block` — the queries rely on positional
/// binding, not `FromRow`, so a shuffle here must be reflected there.
///
/// Resolves `author_id` → bytes by joining `authors` (see
/// `FROM_BLOCKS_JOIN_AUTHORS`). NULL `author_id` keeps the row visible
/// with `author = NULL` (inherent-only / genesis blocks).
const BLOCK_COLUMNS: &str =
    "b.num, b.hash, b.parent_hash, b.state_root, b.extrinsics_root, a.bytes AS author, \
     b.timestamp_ms, b.spec_version, b.extrinsic_count, b.event_count, \
     b.ref_time, b.proof_size, b.ref_time_pct, b.size_bytes";

/// FROM + JOIN clause shared by every read. Isolated so the WHERE
/// clause below stays focused on filters — and so the JOIN form stays
/// in one place, easy to drop if we ever denormalise `author` back onto
/// `blocks`.
const FROM_BLOCKS: &str = "FROM blocks b LEFT JOIN authors a ON a.id = b.author_id";

type BlockRow = (
    i64,             // num
    Vec<u8>,         // hash
    Vec<u8>,         // parent_hash
    Vec<u8>,         // state_root
    Vec<u8>,         // extrinsics_root
    Option<Vec<u8>>, // author
    i64,             // timestamp_ms
    i32,             // spec_version
    i32,             // extrinsic_count
    i32,             // event_count
    i64,             // ref_time
    i64,             // proof_size
    i16,             // ref_time_pct
    i32,             // size_bytes
);

/// Highest block number persisted for `network_id`. `None` when the
/// network hasn't seen a block yet. The status endpoint uses this;
/// the UI's `head_block` still reads the chain's finalized head from
/// the RPC watch so pagination bounds stay correct even while the
/// indexer is catching up.
pub async fn indexed_head(pool: &PgPool, network_sid: i16) -> DataResult<Option<u64>> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT MAX(num) FROM blocks WHERE network_id = $1")
            .bind(network_sid)
            .fetch_optional(pool)
            .await
            .map_err(|e| DataError::Rpc(format!("indexed_head(net={network_sid}): {e}")))?;
    Ok(row.and_then(|(opt,)| opt).map(|v| v as u64))
}

/// Fetch one block by its chain number on `network_sid`. `None` when the
/// row is missing — the caller surfaces that as "block not yet indexed"
/// in the UI; we never fall back to RPC here, that would mask real
/// indexer lag bugs.
pub async fn block_by_number(
    pool: &PgPool,
    network_sid: i16,
    num: u64,
    ss58_prefix: u16,
) -> DataResult<Option<Block>> {
    let sql =
        format!("SELECT {BLOCK_COLUMNS} {FROM_BLOCKS} WHERE b.network_id = $1 AND b.num = $2");
    let row: Option<BlockRow> = sqlx::query_as(&sql)
        .bind(network_sid)
        .bind(num as i64)
        .fetch_optional(pool)
        .await
        .map_err(|e| DataError::Rpc(format!("block_by_number(net={network_sid}/{num}): {e}")))?;
    Ok(row.map(|r| row_to_block(r, ss58_prefix)))
}

/// `count` newest blocks for `network_sid`, ending at `from` if provided
/// (inclusive). Rows are returned newest-first to match the UI's feed
/// order. This is the low-level, vec-returning variant used by
/// [`IndexedProvider`] when it tops up a pending slice — callers that
/// want the full [`Page<Block>`] envelope should go through
/// [`list_blocks_page`] instead.
pub async fn latest_blocks(
    pool: &PgPool,
    network_sid: i16,
    count: u32,
    from: Option<u64>,
    ss58_prefix: u16,
) -> DataResult<Vec<Block>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let rows: Vec<BlockRow> = match from {
        Some(from) => {
            let sql = format!(
                "SELECT {BLOCK_COLUMNS} {FROM_BLOCKS} \
                 WHERE b.network_id = $1 AND b.num <= $2 \
                 ORDER BY b.num DESC LIMIT $3"
            );
            sqlx::query_as(&sql)
                .bind(network_sid)
                .bind(from as i64)
                .bind(count as i64)
                .fetch_all(pool)
                .await
        }
        None => {
            let sql = format!(
                "SELECT {BLOCK_COLUMNS} {FROM_BLOCKS} \
                 WHERE b.network_id = $1 \
                 ORDER BY b.num DESC LIMIT $2"
            );
            sqlx::query_as(&sql)
                .bind(network_sid)
                .bind(count as i64)
                .fetch_all(pool)
                .await
        }
    }
    .map_err(|e| {
        DataError::Rpc(format!(
            "latest_blocks(net={network_sid}, {count}, {from:?}): {e}"
        ))
    })?;
    Ok(rows
        .into_iter()
        .map(|r| row_to_block(r, ss58_prefix))
        .collect())
}

/// DB-only [`Page<Block>`] query honouring the cursor pagination contract.
///
/// The cursor is exclusive: when present, rows with `num < cursor.block`
/// are returned. `filters` (Phase 3) layers `finalized` and
/// `min_extrinsics` on top of the cursor window; see
/// [`list_blocks_page_bounded`] for the WHERE-clause assembly. Internally
/// runs the fetch-N+1 trick so [`PageInfo::has_more`] is set without an
/// extra `COUNT(*)`.
///
/// `total` is left unset here: [`IndexedProvider`] owns the chain-head
/// reference and plugs it in after the fact so the total reflects the
/// live tip (including non-finalized blocks), not the indexer cursor.
pub async fn list_blocks_page(
    pool: &PgPool,
    network_sid: i16,
    req: &PageRequest,
    filters: &BlockFilters,
    ss58_prefix: u16,
) -> DataResult<Page<Block>> {
    let cursor = parse_cursor::<BlockCursor>(req)?;
    list_blocks_page_bounded(
        pool,
        network_sid,
        req.count,
        cursor,
        None,
        filters,
        ss58_prefix,
    )
    .await
}

/// Variant used by [`IndexedProvider`] when a pending slice has already
/// been peeled off the tip. `upper_bound` caps the window from above
/// (inclusive) so the DB query never returns rows the pending slice
/// also owned. Keeps the fetch-N+1 contract.
///
/// `filters.finalized == Some(false)` short-circuits to an empty page:
/// the indexer only persists finalized blocks, so that query has no
/// matching rows by construction. Skipping the round-trip avoids a
/// useless `WHERE FALSE` scan.
pub async fn list_blocks_page_bounded(
    pool: &PgPool,
    network_sid: i16,
    count: u32,
    cursor: Option<BlockCursor>,
    upper_bound: Option<u64>,
    filters: &BlockFilters,
    ss58_prefix: u16,
) -> DataResult<Page<Block>> {
    if count == 0 {
        return Ok(Page::empty());
    }
    if filters.finalized == Some(false) {
        // See doc above — nothing non-finalized ever reaches the DB.
        return Ok(Page::empty());
    }

    // Fetch-N+1: asking for `count + 1` lets the presence of the extra row
    // set `has_more` without an additional `COUNT(*)`. Saturate to stay safe
    // if a caller ever passes `u32::MAX`.
    let probe_count = count.saturating_add(1) as i64;

    // Collapse the cursor + upper-bound combo to a single `num <=` bound
    // so the composite index is probed one way regardless of the caller.
    // `None` stays unbounded (starts at the tip).
    let num_bound: Option<i64> = match (cursor, upper_bound) {
        (Some(c), Some(upper)) => Some(upper.min(c.block.saturating_sub(1)) as i64),
        (Some(c), None) => Some(c.block.saturating_sub(1) as i64),
        (None, Some(upper)) => Some(upper as i64),
        (None, None) => None,
    };

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("SELECT ");
    qb.push(BLOCK_COLUMNS);
    qb.push(" ");
    qb.push(FROM_BLOCKS);
    qb.push(" WHERE b.network_id = ");
    qb.push_bind(network_sid);
    if let Some(bound) = num_bound {
        qb.push(" AND b.num <= ");
        qb.push_bind(bound);
    }
    if let Some(min) = filters.min_extrinsics {
        qb.push(" AND b.extrinsic_count >= ");
        qb.push_bind(min as i32);
    }
    qb.push(" ORDER BY b.num DESC LIMIT ");
    qb.push_bind(probe_count);

    let rows: Vec<BlockRow> = qb.build_query_as().fetch_all(pool).await.map_err(|e| {
        DataError::Rpc(format!(
            "list_blocks_page(net={network_sid}, count={count}, cursor={cursor:?}, filters={filters:?}): {e}"
        ))
    })?;
    Ok(rows_to_page(rows, count, ss58_prefix))
}

/// Turn up to `count + 1` DB rows into a [`Page<Block>`] honouring the
/// fetch-N+1 contract. `total` is left `None`; the provider layer
/// attaches the head-derived value.
pub(crate) fn rows_to_page(mut rows: Vec<BlockRow>, count: u32, ss58_prefix: u16) -> Page<Block> {
    let has_more = rows.len() > count as usize;
    if has_more {
        rows.truncate(count as usize);
    }
    let next_cursor = if has_more {
        rows.last().map(|row| {
            BlockCursor {
                block: row.0.max(0) as u64,
            }
            .to_string()
        })
    } else {
        None
    };
    let items: Vec<Block> = rows
        .into_iter()
        .map(|r| row_to_block(r, ss58_prefix))
        .collect();
    Page {
        items,
        page_info: PageInfo {
            total: None,
            next_cursor,
            has_more,
        },
    }
}

/// Turn a DB row into the domain shape. Finalised is always `true`: the
/// indexer only writes finalised blocks.
fn row_to_block(row: BlockRow, ss58_prefix: u16) -> Block {
    let (
        num,
        hash,
        parent_hash,
        state_root,
        extrinsics_root,
        author,
        timestamp_ms,
        spec_version,
        extrinsic_count,
        event_count,
        ref_time,
        proof_size,
        ref_time_pct,
        size_bytes,
    ) = row;

    let (author_ss58, author_name) = match author.and_then(|b| encode_ss58_bytes(&b, ss58_prefix)) {
        Some(ss58) => {
            let label = short_label(&ss58);
            (ss58, label)
        }
        None => (String::from("unknown"), String::from("unknown")),
    };

    Block {
        number: num as u64,
        hash: hex_bytes(&hash),
        parent_hash: hex_bytes(&parent_hash),
        state_root: hex_bytes(&state_root),
        extrinsics_root: hex_bytes(&extrinsics_root),
        timestamp_ms,
        finalized: true,
        extrinsic_count: extrinsic_count.max(0) as u32,
        event_count: event_count.max(0) as u32,
        author: author_ss58,
        author_name,
        ref_time: ref_time.max(0) as u64,
        ref_time_pct: ref_time_pct.clamp(0, 100) as u8,
        proof_size: proof_size.max(0) as u64,
        spec_version: spec_version.max(0) as u32,
        size_bytes: size_bytes.max(0) as u32,
    }
}
