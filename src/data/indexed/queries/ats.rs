//! SQL reads backing the ATS-related `ChainData` methods.
//!
//! Two tables carry the full state: `ats_registry` (one row per live
//! ATS) and `ats_versions` (one row per historical version, newest
//! version of a given ATS being `MAX(version)`). Every read JOINs both
//! plus the `extrinsics` and `blocks` tables to populate the
//! `AtsVersion` domain shape (signer, fee, timestamp).
//!
//! **Deposits are not event-derived.** The pallet stores them on the
//! `AtsRecord` storage value; the indexer doesn't scan storage per
//! block. Each version costs a fixed reserve of
//! [`crate::domain::VERSION_DEPOSIT`] planck, always paid by the
//! owner in the common case, so the query layer synthesises a single
//! `Deposit { address: owner, amount: version_count × VERSION_DEPOSIT }`
//! entry. A future multi-operator workflow (operators paying the
//! deposit) would need a dedicated projection — flagged in the Phase 6
//! plan but deferred beyond the MVP.

use std::collections::HashMap;

use sqlx::PgPool;
use subxt::utils::AccountId32;

use crate::data::cursor::{parse_cursor, AtsCursor, AtsFeedCursor};
use crate::data::error::{DataError, DataResult};
use crate::data::filters::{AccountAtsFilters, AtsFeedFilters, AtsFilters};
use crate::data::rpc::mappers::hex_bytes;
use crate::data::ss58::encode_ss58_bytes;
use crate::domain::{
    AtsFeedItem, AtsRecord, AtsStats, AtsVersion, Deposit, Page, PageInfo, PageRequest,
    VERSION_DEPOSIT,
};

/// Column list shared by every version read. Kept in sync with the
/// tuple destructure in [`row_to_version`] — positional binding, not
/// `FromRow`, so a column shuffle here must be reflected there.
const VERSION_COLUMNS: &str = "v.ats_id, v.version, v.block_num, v.ext_idx, \
                               v.commitment, v.protocol_version, \
                               b.timestamp_ms, \
                               x.fee::text, x.signer";

/// Raw row tuple for a `ats_versions` read, JOINed with `blocks` and
/// `extrinsics`.
type VersionRow = (
    i64,             // ats_id
    i32,             // version
    i64,             // block_num
    i32,             // ext_idx
    Option<Vec<u8>>, // commitment
    Option<i16>,     // protocol_version
    Option<i64>,     // b.timestamp_ms (LEFT JOIN — block row may be absent on partial backfill)
    Option<String>,  // x.fee::text (LEFT JOIN — extrinsic may not be indexed yet)
    Option<Vec<u8>>, // x.signer
);

/// Raw row tuple for the version-feed read. Packaged as a type alias
/// so the `sqlx::query_as` bind site doesn't trip the "very complex
/// type" lint; decode is done in [`feed_row_to_item`].
type FeedRow = (
    i64,             // ats_id
    Vec<u8>,         // owner
    i32,             // version_count
    i32,             // version
    Option<Vec<u8>>, // commitment
    Option<i16>,     // protocol_version
    i64,             // block_num
    i32,             // ext_idx
    Option<i64>,     // timestamp_ms
    Option<Vec<u8>>, // signer
);

/// One ATS row + its owner, stripped to the columns the list / feed /
/// detail queries all need. Packing it into a helper type lets every
/// query above share the JOIN + the decode loop.
type RegistryRow = (
    i64,     // id
    Vec<u8>, // owner
    i64,     // created_block
    i32,     // version_count
    i64,     // created_timestamp_ms (JOIN blocks on created_block)
);

/// Highest ATS id indexed on `network_id`. `None` on a fresh DB —
/// callers treat that as "no ATS activity yet" and return empty feeds.
pub async fn max_ats_id(pool: &PgPool, network_sid: i16) -> DataResult<Option<u64>> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT MAX(id) FROM ats_registry WHERE network_id = $1")
            .bind(network_sid)
            .fetch_optional(pool)
            .await
            .map_err(|e| DataError::Rpc(format!("max_ats_id(net={network_sid}): {e}")))?;
    Ok(row.and_then(|(opt,)| opt).map(|v| v as u64))
}

/// Fetch one ATS on `network_id` by its **chain `ats_id`**.
///
/// Returns `None` when the id is unknown or the entry has been revoked
/// (the registry row is deleted by `AtsRevoked` and the cascading
/// version delete drops the history with it).
pub async fn ats_by_id(
    pool: &PgPool,
    network_sid: i16,
    ats_id: u32,
    ss58_prefix: u16,
) -> DataResult<Option<AtsRecord>> {
    let registry: Option<RegistryRow> = sqlx::query_as(
        "SELECT r.id, r.owner, r.created_block, r.version_count, \
                COALESCE(b.timestamp_ms, 0) AS created_timestamp_ms \
         FROM ats_registry r \
         LEFT JOIN blocks b ON b.network_id = r.network_id AND b.num = r.created_block \
         WHERE r.network_id = $1 AND r.id = $2 \
         LIMIT 1",
    )
    .bind(network_sid)
    .bind(ats_id as i64)
    .fetch_optional(pool)
    .await
    .map_err(|e| DataError::Rpc(format!("ats_by_id(net={network_sid}, {ats_id}): {e}")))?;

    let Some(reg) = registry else {
        return Ok(None);
    };

    let versions = versions_for_ats(pool, network_sid, reg.0, ss58_prefix).await?;
    Ok(Some(registry_to_record(reg, versions, ss58_prefix)))
}

/// Newest-first list of ATS records on `network_id`, paginated by
/// [`AtsCursor`] (the ATS id). Trades the old offset-based API for the
/// cursor contract so a new registration during pagination can't shift
/// rows out of the window.
pub async fn list_ats_page(
    pool: &PgPool,
    network_sid: i16,
    req: &PageRequest,
    _filters: &AtsFilters,
    ss58_prefix: u16,
) -> DataResult<Page<AtsRecord>> {
    if req.count == 0 {
        return Ok(Page::empty());
    }
    let cursor = parse_cursor::<AtsCursor>(req)?;
    let probe = (req.count as i64).saturating_add(1);

    let rows: Vec<RegistryRow> = match cursor {
        Some(c) => {
            sqlx::query_as(
                "SELECT r.id, r.owner, r.created_block, r.version_count, \
                    COALESCE(b.timestamp_ms, 0) AS created_timestamp_ms \
             FROM ats_registry r \
             LEFT JOIN blocks b ON b.network_id = r.network_id AND b.num = r.created_block \
             WHERE r.network_id = $1 AND r.id < $2 \
             ORDER BY r.id DESC \
             LIMIT $3",
            )
            .bind(network_sid)
            .bind(c.id as i64)
            .bind(probe)
            .fetch_all(pool)
            .await
        }
        None => {
            sqlx::query_as(
                "SELECT r.id, r.owner, r.created_block, r.version_count, \
                    COALESCE(b.timestamp_ms, 0) AS created_timestamp_ms \
             FROM ats_registry r \
             LEFT JOIN blocks b ON b.network_id = r.network_id AND b.num = r.created_block \
             WHERE r.network_id = $1 \
             ORDER BY r.id DESC \
             LIMIT $2",
            )
            .bind(network_sid)
            .bind(probe)
            .fetch_all(pool)
            .await
        }
    }
    .map_err(|e| {
        DataError::Rpc(format!(
            "list_ats_page(net={network_sid}, count={}): {e}",
            req.count
        ))
    })?;

    let has_more = rows.len() > req.count as usize;
    let mut truncated = rows;
    if has_more {
        truncated.truncate(req.count as usize);
    }
    // Compute the cursor before consuming the rows into `AtsRecord`s.
    let next_cursor = if has_more {
        truncated.last().map(|r| {
            AtsCursor {
                id: ats_id_u32(r.0),
            }
            .to_string()
        })
    } else {
        None
    };

    let items = hydrate_records(pool, network_sid, truncated, ss58_prefix).await?;

    // Pull `total` from ats_stats — it's cheap (single-row aggregate
    // already computed for the stats strip) and the UI uses it.
    let total = registry_total(pool, network_sid).await.ok();

    Ok(Page {
        items,
        page_info: PageInfo {
            total,
            next_cursor,
            has_more,
        },
    })
}

/// Flat version feed across every ATS on `network_id`, ordered by
/// `(ats_id DESC, version DESC)` — which is the ordering the
/// [`AtsFeedCursor`] grammar expects (`(id, version) < cursor` lex).
/// A given ATS's versions descend together; a newer ATS sorts above an
/// older one even if the older one just got a new version. That's a
/// tiny ordering divergence from strict block-time newest-first, but
/// keeps cursor arithmetic monotonic and is what the plan prescribes.
pub async fn list_ats_feed_page(
    pool: &PgPool,
    network_sid: i16,
    req: &PageRequest,
    _filters: &AtsFeedFilters,
    ss58_prefix: u16,
) -> DataResult<Page<AtsFeedItem>> {
    if req.count == 0 {
        return Ok(Page::empty());
    }
    let cursor = parse_cursor::<AtsFeedCursor>(req)?;
    let probe = (req.count as i64).saturating_add(1);

    let rows: Vec<FeedRow> = match cursor {
        Some(c) => sqlx::query_as(
            "SELECT v.ats_id, r.owner, r.version_count, \
                    v.version, v.commitment, v.protocol_version, \
                    v.block_num, v.ext_idx, \
                    b.timestamp_ms, x.signer \
             FROM ats_versions v \
             JOIN ats_registry r ON r.network_id = v.network_id AND r.id = v.ats_id \
             LEFT JOIN blocks b ON b.network_id = v.network_id AND b.num = v.block_num \
             LEFT JOIN extrinsics x ON x.network_id = v.network_id AND x.block_num = v.block_num AND x.idx = v.ext_idx \
             WHERE v.network_id = $1 AND (v.ats_id, v.version) < ($2, $3) \
             ORDER BY v.ats_id DESC, v.version DESC \
             LIMIT $4",
        )
        .bind(network_sid)
        .bind(c.ats_id as i64)
        .bind(c.version as i32)
        .bind(probe)
        .fetch_all(pool)
        .await,
        None => sqlx::query_as(
            "SELECT v.ats_id, r.owner, r.version_count, \
                    v.version, v.commitment, v.protocol_version, \
                    v.block_num, v.ext_idx, \
                    b.timestamp_ms, x.signer \
             FROM ats_versions v \
             JOIN ats_registry r ON r.network_id = v.network_id AND r.id = v.ats_id \
             LEFT JOIN blocks b ON b.network_id = v.network_id AND b.num = v.block_num \
             LEFT JOIN extrinsics x ON x.network_id = v.network_id AND x.block_num = v.block_num AND x.idx = v.ext_idx \
             WHERE v.network_id = $1 \
             ORDER BY v.ats_id DESC, v.version DESC \
             LIMIT $2",
        )
        .bind(network_sid)
        .bind(probe)
        .fetch_all(pool)
        .await,
    }
    .map_err(|e| DataError::Rpc(format!("list_ats_feed_page(net={network_sid}): {e}")))?;

    let mut probed: Vec<AtsFeedItem> = rows
        .into_iter()
        .filter_map(|r| feed_row_to_item(r, ss58_prefix))
        .collect();
    let has_more = probed.len() > req.count as usize;
    if has_more {
        probed.truncate(req.count as usize);
    }
    let next_cursor = if has_more {
        probed.last().map(|item| {
            AtsFeedCursor {
                ats_id: item.ats_id,
                version: item.version_index,
            }
            .to_string()
        })
    } else {
        None
    };

    // `total` = SUM(version_count) across the registry. Reused from the
    // stats aggregate; falls back to `None` on error so a misbehaving
    // aggregate doesn't break the list.
    let total = version_total(pool, network_sid).await.ok();

    Ok(Page {
        items: probed,
        page_info: PageInfo {
            total,
            next_cursor,
            has_more,
        },
    })
}

/// Old offset-based variant, kept for the mock path and any internal
/// caller that still needs a plain `Vec`. Prefer [`list_ats_feed_page`].
pub async fn ats_version_feed(
    pool: &PgPool,
    network_sid: i16,
    count: u32,
    from_index: u32,
    ss58_prefix: u16,
) -> DataResult<Vec<AtsFeedItem>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let rows: Vec<FeedRow> = sqlx::query_as(
        "SELECT v.ats_id, r.owner, r.version_count, \
                v.version, v.commitment, v.protocol_version, \
                v.block_num, v.ext_idx, \
                b.timestamp_ms, x.signer \
         FROM ats_versions v \
         JOIN ats_registry r ON r.network_id = v.network_id AND r.id = v.ats_id \
         LEFT JOIN blocks b ON b.network_id = v.network_id AND b.num = v.block_num \
         LEFT JOIN extrinsics x ON x.network_id = v.network_id AND x.block_num = v.block_num AND x.idx = v.ext_idx \
         WHERE v.network_id = $1 \
         ORDER BY v.block_num DESC, v.ats_id DESC, v.version DESC \
         OFFSET $2 LIMIT $3",
    )
    .bind(network_sid)
    .bind(from_index as i64)
    .bind(count as i64)
    .fetch_all(pool)
    .await
    .map_err(|e| {
        DataError::Rpc(format!(
            "ats_version_feed(net={network_sid}, {count},{from_index}): {e}"
        ))
    })?;

    Ok(rows
        .into_iter()
        .filter_map(|r| feed_row_to_item(r, ss58_prefix))
        .collect())
}

fn feed_row_to_item(row: FeedRow, ss58_prefix: u16) -> Option<AtsFeedItem> {
    let (
        ats_id,
        owner_bytes,
        version_count,
        version,
        commitment,
        protocol_version,
        block_num,
        ext_idx,
        timestamp_ms,
        signer_bytes,
    ) = row;
    let owner = encode_ss58_bytes(&owner_bytes, ss58_prefix)?;
    let block_number = block_num.max(0) as u64;
    let extrinsic_index = ext_idx.max(0) as u32;
    let signer = signer_bytes
        .as_deref()
        .and_then(|b| encode_ss58_bytes(b, ss58_prefix))
        .unwrap_or_else(|| owner.clone());
    let version_count = version_count.max(0) as u32;
    let version_index = version.max(0) as u32;
    Some(AtsFeedItem {
        ats_id: ats_id_u32(ats_id),
        owner,
        version_index,
        is_initial: version_index == 0,
        is_latest: version_index + 1 == version_count,
        version_count,
        commitment: commitment.as_deref().map(hex_bytes).unwrap_or_default(),
        protocol_version: protocol_version.unwrap_or(0).max(0) as u8,
        block_number,
        extrinsic_id: format!("{block_number}-{extrinsic_index}"),
        timestamp_ms: timestamp_ms.unwrap_or(0),
        signer,
    })
}

/// Paginated newest-first ATS list for a given owner. Same cursor shape
/// as [`list_ats_page`] (the ATS id) — the frontend binds to one
/// `AtsCursor` type regardless of whether it's scoped by owner or not.
pub async fn list_account_ats_page(
    pool: &PgPool,
    network_sid: i16,
    address: &str,
    req: &PageRequest,
    _filters: &AccountAtsFilters,
    ss58_prefix: u16,
) -> DataResult<Page<AtsRecord>> {
    if req.count == 0 {
        return Ok(Page::empty());
    }
    let Some(owner) = address.parse::<AccountId32>().ok() else {
        return Ok(Page::empty());
    };
    let owner_bytes: &[u8; 32] = owner.as_ref();
    let cursor = parse_cursor::<AtsCursor>(req)?;
    let probe = (req.count as i64).saturating_add(1);

    let rows: Vec<RegistryRow> = match cursor {
        Some(c) => {
            sqlx::query_as(
                "SELECT r.id, r.owner, r.created_block, r.version_count, \
                    COALESCE(b.timestamp_ms, 0) AS created_timestamp_ms \
             FROM ats_registry r \
             LEFT JOIN blocks b ON b.network_id = r.network_id AND b.num = r.created_block \
             WHERE r.network_id = $1 AND r.owner = $2 AND r.id < $3 \
             ORDER BY r.id DESC \
             LIMIT $4",
            )
            .bind(network_sid)
            .bind(&owner_bytes[..])
            .bind(c.id as i64)
            .bind(probe)
            .fetch_all(pool)
            .await
        }
        None => {
            sqlx::query_as(
                "SELECT r.id, r.owner, r.created_block, r.version_count, \
                    COALESCE(b.timestamp_ms, 0) AS created_timestamp_ms \
             FROM ats_registry r \
             LEFT JOIN blocks b ON b.network_id = r.network_id AND b.num = r.created_block \
             WHERE r.network_id = $1 AND r.owner = $2 \
             ORDER BY r.id DESC \
             LIMIT $3",
            )
            .bind(network_sid)
            .bind(&owner_bytes[..])
            .bind(probe)
            .fetch_all(pool)
            .await
        }
    }
    .map_err(|e| {
        DataError::Rpc(format!(
            "list_account_ats_page(net={network_sid}/{address}, count={}): {e}",
            req.count
        ))
    })?;

    let has_more = rows.len() > req.count as usize;
    let mut truncated = rows;
    if has_more {
        truncated.truncate(req.count as usize);
    }
    let next_cursor = if has_more {
        truncated.last().map(|r| {
            AtsCursor {
                id: ats_id_u32(r.0),
            }
            .to_string()
        })
    } else {
        None
    };

    let items = hydrate_records(pool, network_sid, truncated, ss58_prefix).await?;

    // Cheap: the count scan is already indexed on `(network_id, owner)`.
    let total = account_ats_count(pool, network_sid, address)
        .await
        .ok()
        .map(u64::from);

    Ok(Page {
        items,
        page_info: PageInfo {
            total,
            next_cursor,
            has_more,
        },
    })
}

/// Retained `Vec`-returning variant for internal callers (mock backfill,
/// tests). External HTTP flows go through [`list_account_ats_page`].
pub async fn account_ats(
    pool: &PgPool,
    network_sid: i16,
    address: &str,
    limit: u32,
    ss58_prefix: u16,
) -> DataResult<Vec<AtsRecord>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let Some(owner) = address.parse::<AccountId32>().ok() else {
        return Ok(Vec::new());
    };
    let owner_bytes: &[u8; 32] = owner.as_ref();

    let rows: Vec<RegistryRow> = sqlx::query_as(
        "SELECT r.id, r.owner, r.created_block, r.version_count, \
                COALESCE(b.timestamp_ms, 0) AS created_timestamp_ms \
         FROM ats_registry r \
         LEFT JOIN blocks b ON b.network_id = r.network_id AND b.num = r.created_block \
         WHERE r.network_id = $1 AND r.owner = $2 \
         ORDER BY r.id DESC \
         LIMIT $3",
    )
    .bind(network_sid)
    .bind(&owner_bytes[..])
    .bind(limit as i64)
    .fetch_all(pool)
    .await
    .map_err(|e| {
        DataError::Rpc(format!(
            "account_ats(net={network_sid}/{address},{limit}): {e}"
        ))
    })?;

    hydrate_records(pool, network_sid, rows, ss58_prefix).await
}

/// Number of ATS entries on `network_id` owned by `address`.
pub async fn account_ats_count(pool: &PgPool, network_sid: i16, address: &str) -> DataResult<u32> {
    let Some(owner) = address.parse::<AccountId32>().ok() else {
        return Ok(0);
    };
    let owner_bytes: &[u8; 32] = owner.as_ref();
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ats_registry WHERE network_id = $1 AND owner = $2",
    )
    .bind(network_sid)
    .bind(&owner_bytes[..])
    .fetch_one(pool)
    .await
    .map_err(|e| {
        DataError::Rpc(format!(
            "account_ats_count(net={network_sid}/{address}): {e}"
        ))
    })?;
    Ok(count.max(0) as u32)
}

/// Aggregate stats over the full registry for `network_id`. Single
/// round-trip: a CTE computes every aggregate (registry totals,
/// ownership, time-window counts, genesis lookup) in one pass so the
/// stats strip doesn't cost seven sequential queries. The version
/// windows share `now_ms` via a correlated subquery instead of being
/// recomputed per branch — the planner collapses them into a single
/// scan of `ats_versions ⋈ blocks`.
pub async fn ats_stats(pool: &PgPool, network_sid: i16) -> DataResult<AtsStats> {
    const H24_MS: i64 = 24 * 60 * 60 * 1000;
    const D7_MS: i64 = 7 * H24_MS;
    const D30_MS: i64 = 30 * H24_MS;

    // Column order below must match the SELECT list.
    type Row = (
        i64,         // total
        i64,         // multi_version
        i64,         // total_versions
        i64,         // unique_owners
        Option<i64>, // genesis_block
        Option<i64>, // genesis_ts_ms
        Option<i64>, // now_ms
        i64,         // last_24h
        i64,         // last_7d
        i64,         // last_30d
    );

    let row: Option<Row> = sqlx::query_as(
        "WITH \
         now_ts AS ( \
             SELECT MAX(timestamp_ms) AS ms FROM blocks WHERE network_id = $1 \
         ), \
         reg AS ( \
             SELECT \
                 COUNT(*)::bigint AS total, \
                 COUNT(*) FILTER (WHERE r.version_count > 1)::bigint AS multi_version, \
                 COALESCE(SUM(r.version_count), 0)::bigint AS total_versions, \
                 COUNT(DISTINCT r.owner)::bigint AS unique_owners, \
                 MIN(r.created_block) AS genesis_block \
             FROM ats_registry r \
             WHERE r.network_id = $1 \
         ), \
         ver AS ( \
             SELECT \
                 COUNT(*) FILTER (WHERE b.timestamp_ms >= (SELECT ms FROM now_ts) - $2)::bigint AS last_24h, \
                 COUNT(*) FILTER (WHERE b.timestamp_ms >= (SELECT ms FROM now_ts) - $3)::bigint AS last_7d, \
                 COUNT(*) FILTER (WHERE b.timestamp_ms >= (SELECT ms FROM now_ts) - $4)::bigint AS last_30d \
             FROM ats_versions v \
             JOIN blocks b ON b.network_id = v.network_id AND b.num = v.block_num \
             WHERE v.network_id = $1 \
         ) \
         SELECT \
             reg.total, reg.multi_version, reg.total_versions, reg.unique_owners, \
             reg.genesis_block, \
             (SELECT timestamp_ms FROM blocks \
                WHERE network_id = $1 AND num = reg.genesis_block) AS genesis_ts_ms, \
             (SELECT ms FROM now_ts) AS now_ms, \
             ver.last_24h, ver.last_7d, ver.last_30d \
         FROM reg, ver",
    )
    .bind(network_sid)
    .bind(H24_MS)
    .bind(D7_MS)
    .bind(D30_MS)
    .fetch_optional(pool)
    .await
    .map_err(|e| DataError::Rpc(format!("ats_stats(net={network_sid}): {e}")))?;

    let (
        total,
        multi_version,
        total_versions,
        unique_owners,
        genesis_block,
        genesis_ts_ms,
        _now_ms,
        last_24h,
        last_7d,
        last_30d,
    ) = row.unwrap_or((0, 0, 0, 0, None, None, None, 0, 0, 0));

    let total = total.max(0) as u32;
    let total_versions = total_versions.max(0) as u32;
    let unique_owners = unique_owners.max(0) as u32;
    let multi_version = multi_version.max(0) as u32;
    let last_24h = last_24h.max(0) as u32;
    let last_7d = last_7d.max(0) as u32;
    let last_30d = last_30d.max(0) as u32;
    let genesis_block = genesis_block.unwrap_or(0).max(0) as u64;
    let first_registered_at_ms = genesis_ts_ms.unwrap_or(0);

    let multi_version_share = if total == 0 {
        0.0
    } else {
        multi_version as f32 / total as f32
    };

    Ok(AtsStats {
        total,
        total_versions,
        last_24h,
        last_7d,
        last_30d,
        unique_owners,
        avg_per_day: last_24h,
        // pallet-ats is v1 today; matches the RPC path which hard-codes
        // the same value. If a runtime upgrade ever changes the protocol
        // we'll need to surface it through an event — not worth a
        // separate query for a constant.
        protocol_version: 1,
        genesis_block,
        first_registered_at_ms,
        // Every live version reserves `VERSION_DEPOSIT`. Summing over
        // `version_count` matches the on-chain invariant without
        // storing a dedicated deposit column.
        total_deposited: (total_versions as u128).saturating_mul(VERSION_DEPOSIT),
        multi_version_share,
    })
}

/// Pull every version row for `ats_id` on `network_id`.
async fn versions_for_ats(
    pool: &PgPool,
    network_sid: i16,
    ats_id: i64,
    ss58_prefix: u16,
) -> DataResult<Vec<AtsVersion>> {
    let rows: Vec<VersionRow> = sqlx::query_as(&format!(
        "SELECT {VERSION_COLUMNS} \
             FROM ats_versions v \
             LEFT JOIN blocks b ON b.network_id = v.network_id AND b.num = v.block_num \
             LEFT JOIN extrinsics x ON x.network_id = v.network_id AND x.block_num = v.block_num AND x.idx = v.ext_idx \
             WHERE v.network_id = $1 AND v.ats_id = $2 \
             ORDER BY v.version ASC",
    ))
    .bind(network_sid)
    .bind(ats_id)
    .fetch_all(pool)
    .await
    .map_err(|e| DataError::Rpc(format!("versions_for_ats(net={network_sid}/{ats_id}): {e}")))?;
    Ok(rows
        .into_iter()
        .filter_map(|r| row_to_version(r, ss58_prefix))
        .collect())
}

/// Batch-load versions for every registry row in `regs`.
async fn hydrate_records(
    pool: &PgPool,
    network_sid: i16,
    regs: Vec<RegistryRow>,
    ss58_prefix: u16,
) -> DataResult<Vec<AtsRecord>> {
    if regs.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = regs.iter().map(|r| r.0).collect();
    let rows: Vec<VersionRow> = sqlx::query_as(&format!(
        "SELECT {VERSION_COLUMNS} \
             FROM ats_versions v \
             LEFT JOIN blocks b ON b.network_id = v.network_id AND b.num = v.block_num \
             LEFT JOIN extrinsics x ON x.network_id = v.network_id AND x.block_num = v.block_num AND x.idx = v.ext_idx \
             WHERE v.network_id = $1 AND v.ats_id = ANY($2) \
             ORDER BY v.ats_id ASC, v.version ASC",
    ))
    .bind(network_sid)
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|e| DataError::Rpc(format!("hydrate_records(net={network_sid}) versions: {e}")))?;

    let mut by_id: HashMap<i64, Vec<AtsVersion>> = HashMap::new();
    for row in rows {
        let ats_id = row.0;
        if let Some(v) = row_to_version(row, ss58_prefix) {
            by_id.entry(ats_id).or_default().push(v);
        }
    }

    Ok(regs
        .into_iter()
        .map(|reg| {
            let versions = by_id.remove(&reg.0).unwrap_or_default();
            registry_to_record(reg, versions, ss58_prefix)
        })
        .collect())
}

/// Build the final `AtsRecord` out of a registry row + its hydrated
/// versions. Version list may be empty when a partially-indexed DB
/// carries the registry row but not yet any version row — surface the
/// record anyway so operators can see what's in flight.
fn registry_to_record(reg: RegistryRow, versions: Vec<AtsVersion>, ss58_prefix: u16) -> AtsRecord {
    let (id, owner_bytes, created_block, version_count, created_ms) = reg;
    let owner =
        encode_ss58_bytes(&owner_bytes, ss58_prefix).unwrap_or_else(|| hex_bytes(&owner_bytes));
    let version_count = version_count.max(0) as u32;

    // Deposits: one synthesised entry per ATS, attributed to the
    // owner. See module docs for the rationale.
    let total_deposit = (version_count as u128).saturating_mul(VERSION_DEPOSIT);
    let deposits = if version_count > 0 {
        vec![Deposit {
            address: owner.clone(),
            amount: total_deposit,
        }]
    } else {
        Vec::new()
    };

    AtsRecord {
        ats_id: ats_id_u32(id),
        owner,
        created_at_ms: created_ms,
        created_at_block: created_block.max(0) as u64,
        version_count,
        deposits,
        total_deposit,
        versions,
    }
}

/// Decode one `ats_versions` row into the domain `AtsVersion`. Falls
/// back to owner-as-signer / zero-fee when the JOINed extrinsic row
/// hasn't landed yet — same partial-state philosophy as
/// [`registry_to_record`]: prefer a partial row over hiding the
/// ATS entirely.
fn row_to_version(row: VersionRow, ss58_prefix: u16) -> Option<AtsVersion> {
    let (
        _ats_id,
        version,
        block_num,
        ext_idx,
        commitment,
        protocol_version,
        timestamp_ms,
        fee_text,
        signer_bytes,
    ) = row;

    let version_index = version.max(0) as u32;
    let block_number = block_num.max(0) as u64;
    let extrinsic_index = ext_idx.max(0) as u32;
    let commitment = commitment
        .as_deref()
        .map(hex_bytes)
        .unwrap_or_else(String::new);
    let protocol_version = protocol_version.unwrap_or(0).max(0) as u8;
    let timestamp = timestamp_ms.unwrap_or(0);
    let fee = fee_text.and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
    let signer = signer_bytes
        .as_deref()
        .and_then(|b| encode_ss58_bytes(b, ss58_prefix))
        .unwrap_or_default();

    Some(AtsVersion {
        version_index,
        commitment,
        protocol_version,
        created_at_ms: timestamp,
        block_number,
        extrinsic_index,
        extrinsic_id: format!("{block_number}-{extrinsic_index}"),
        fee,
        signer,
    })
}

/// Saturate u64 → u32. ATS ids grow from zero and realistic deployments
/// stay well under `u32::MAX`; mirror the RPC mapper's behaviour so the
/// domain type stays u32 everywhere.
fn ats_id_u32(id: i64) -> u32 {
    u32::try_from(id.max(0) as u64).unwrap_or(u32::MAX)
}

/// Total rows in `ats_registry` for `network_id`. Cheap: index-only
/// scan of the primary key. Used by [`list_ats_page`] as the page
/// `total`.
async fn registry_total(pool: &PgPool, network_sid: i16) -> DataResult<u64> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ats_registry WHERE network_id = $1")
        .bind(network_sid)
        .fetch_one(pool)
        .await
        .map_err(|e| DataError::Rpc(format!("registry_total(net={network_sid}): {e}")))?;
    Ok(count.max(0) as u64)
}

/// Sum of `version_count` across the registry — same aggregate already
/// produced by [`ats_stats`]. Used by [`list_ats_feed_page`] for its
/// `total`.
async fn version_total(pool: &PgPool, network_sid: i16) -> DataResult<u64> {
    let total: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(version_count), 0) FROM ats_registry WHERE network_id = $1",
    )
    .bind(network_sid)
    .fetch_one(pool)
    .await
    .map_err(|e| DataError::Rpc(format!("version_total(net={network_sid}): {e}")))?;
    Ok(total.max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Saturation guards the u64 → u32 boundary the plan calls out.
    /// Mirrors `ats_id_u32` in the RPC mapper; keeping both aligned
    /// means the UI sees consistent ids across backends during
    /// migration.
    #[test]
    fn ats_id_u32_saturates_at_u32_max() {
        assert_eq!(ats_id_u32(0), 0);
        assert_eq!(ats_id_u32(42), 42);
        assert_eq!(ats_id_u32(i64::MAX), u32::MAX);
    }
}
