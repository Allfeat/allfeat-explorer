//! ATS pallet mapping: registry fan-out, per-record version walk, stats
//! aggregation, and the per-owner views.

use futures::{stream, StreamExt, TryStreamExt};
use subxt::client::OnlineClientAtBlock;
use subxt::utils::AccountId32;
use subxt::SubstrateConfig;

use crate::data::error::{DataError, DataResult};
use crate::data::rpc::client::{with_iter_timeout, with_timeout};
use crate::data::rpc::runtime::allfeat;
use crate::data::ss58::encode_ss58;
use crate::domain::{AtsFeedItem, AtsRecord, AtsStats, AtsVersion, Deposit};

use super::accounts::parse_ss58;
use super::blocks::fetch_timestamp;
use super::common::{fetch_block_events, hex_bytes, index_events_by_phase};
use super::extrinsics::{extrinsic_id, map_extrinsics};

/// Concurrency cap for the ATS fan-out helpers. Each task reads the ATS
/// registry entry + per-version info + the creator block's events, so 8 keeps
/// a single-threaded dev node busy without overwhelming it. Mirrors
/// `FETCH_CONCURRENCY` in the provider for the block scans.
const ATS_CONCURRENCY: usize = 8;

/// SCALE type for an on-chain ATS registry value. Matches the Melodie runtime's
/// `pallet_ats::AtsRecord<AccountId, BlockNumber, Balance>` instantiation.
type OnChainAtsRecord =
    allfeat::runtime_types::pallet_ats::types::AtsRecord<AccountId32, u32, u128>;
type OnChainVersionInfo = allfeat::runtime_types::pallet_ats::types::VersionInfo<u32>;

/// Fallback/resolved "how did this version land on-chain" info that we need
/// to round-trip from `VersionInfo` (which only stores the commitment + block)
/// to the explorer's richer `AtsVersion` domain struct.
struct VersionExtras {
    timestamp_ms: i64,
    extrinsic_index: u32,
    fee: u128,
    signer: String,
}

/// Pull the auto-incrementing next-ATS-id counter (= total entries ever
/// created, before any revocations). ValueQuery default is `0`.
pub async fn fetch_next_ats_id(at: &OnlineClientAtBlock<SubstrateConfig>) -> DataResult<u64> {
    let value = with_timeout("fetch_next_ats_id", async {
        at.storage()
            .try_fetch(allfeat::storage().ats().next_ats_id(), ())
            .await
            .map_err(|e| DataError::Rpc(format!("fetch next_ats_id: {e}")))
    })
    .await?;
    let Some(value) = value else {
        return Ok(0);
    };
    value
        .decode()
        .map_err(|e| DataError::Decode(format!("decode next_ats_id: {e}")))
}

/// Fetch the on-chain registry entry for `ats_id`, or `None` when the entry
/// was revoked (hard-deleted by the pallet) or never existed.
pub async fn fetch_ats_registry_entry(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    ats_id: u64,
) -> DataResult<Option<OnChainAtsRecord>> {
    let value = with_timeout("fetch_ats_registry_entry", async {
        at.storage()
            .try_fetch(allfeat::storage().ats().ats_registry(), (ats_id,))
            .await
            .map_err(|e| DataError::Rpc(format!("fetch ats_registry({ats_id}): {e}")))
    })
    .await?;
    let Some(value) = value else {
        return Ok(None);
    };
    value
        .decode()
        .map(Some)
        .map_err(|e| DataError::Decode(format!("decode AtsRecord: {e}")))
}

/// Stream the `(version, VersionInfo)` pairs stored under a single ATS id.
/// Ordered ascending by version index so callers can rely on `[0]` being the
/// initial version.
pub async fn fetch_ats_versions(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    ats_id: u64,
) -> DataResult<Vec<(u32, OnChainVersionInfo)>> {
    with_iter_timeout("fetch_ats_versions", async {
        let mut entries = at
            .storage()
            .iter(allfeat::storage().ats().ats_versions(), (ats_id,))
            .await
            .map_err(|e| DataError::Rpc(format!("iter ats_versions({ats_id}): {e}")))?;

        let mut out = Vec::new();
        while let Some(kv) = entries.next().await {
            let kv = kv.map_err(|e| DataError::Rpc(format!("iter ats_versions next: {e}")))?;
            let (_, v) = kv
                .key()
                .map_err(|e| DataError::Decode(format!("ats_versions key: {e}")))?
                .decode()
                .map_err(|e| DataError::Decode(format!("decode ats_versions key: {e}")))?;
            let info = kv
                .value()
                .decode()
                .map_err(|e| DataError::Decode(format!("decode VersionInfo: {e}")))?;
            out.push((v, info));
        }
        out.sort_by_key(|(v, _)| *v);
        Ok(out)
    })
    .await
}

/// Fetch `OwnerIndex[owner]`. ValueQuery default is an empty vec, so a missing
/// entry (owner never owned an ATS) surfaces as `vec![]`.
pub async fn fetch_owner_index(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    owner: &AccountId32,
) -> DataResult<Vec<u64>> {
    let value = with_timeout("fetch_owner_index", async {
        at.storage()
            .try_fetch(allfeat::storage().ats().owner_index(), (*owner,))
            .await
            .map_err(|e| DataError::Rpc(format!("fetch owner_index: {e}")))
    })
    .await?;
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let bounded: allfeat::runtime_types::bounded_collections::bounded_vec::BoundedVec<u64> = value
        .decode()
        .map_err(|e| DataError::Decode(format!("decode owner_index: {e}")))?;
    Ok(bounded.0)
}

/// Resolve the extrinsic extras (fee, signer, index, timestamp) for a version
/// by loading the block it was created at and looking up the `AtsCreated` /
/// `AtsUpdated` event's phase index.
///
/// Falls back to `(0, 0, owner)` when the corresponding event can't be found
/// — older blocks drop out of the chainHead V1 pinned window and we prefer a
/// partial row to an error page.
async fn fetch_version_extras(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    ats_id: u64,
    version: u32,
    owner: &AccountId32,
    ss58_prefix: u16,
) -> DataResult<VersionExtras> {
    let timestamp_ms = fetch_timestamp(at).await?;
    let events = fetch_block_events(at).await?;
    let events_by_phase = index_events_by_phase(&events)?;
    let extrinsics = map_extrinsics(at, timestamp_ms, &events_by_phase, ss58_prefix).await?;

    // Walk the phase-indexed events to find the AtsCreated / AtsUpdated that
    // matches `(ats_id, version)`. The outer key already carries the extrinsic
    // index, so we skip straight to the extrinsic lookup once we've matched.
    let mut extrinsic_index: Option<u32> = None;
    'outer: for (idx, evt_slice) in &events_by_phase {
        for evt in evt_slice {
            let matches = match (evt.pallet_name(), evt.event_name(), version) {
                ("Ats", "AtsCreated", 0) => evt
                    .decode_fields_unchecked_as::<allfeat::ats::events::AtsCreated>()
                    .map(|ev| ev.ats_id == ats_id)
                    .unwrap_or(false),
                ("Ats", "AtsUpdated", v) if v > 0 => evt
                    .decode_fields_unchecked_as::<allfeat::ats::events::AtsUpdated>()
                    .map(|ev| ev.ats_id == ats_id && ev.version == version)
                    .unwrap_or(false),
                _ => false,
            };
            if matches {
                extrinsic_index = Some(*idx);
                break 'outer;
            }
        }
    }

    let (extrinsic_index, fee, signer) = match extrinsic_index {
        Some(idx) => {
            let ext = extrinsics.iter().find(|e| e.index == idx);
            let fee = ext.map(|e| e.fee).unwrap_or(0);
            let signer = ext
                .and_then(|e| e.signer.clone())
                .unwrap_or_else(|| encode_ss58(&owner.0, ss58_prefix));
            (idx, fee, signer)
        }
        None => (0, 0, encode_ss58(&owner.0, ss58_prefix)),
    };

    Ok(VersionExtras {
        timestamp_ms,
        extrinsic_index,
        fee,
        signer,
    })
}

/// Downcast an on-chain u64 `AtsId` to the u32 the domain layer expects.
/// AtsIds grow from zero and caps on a real deployment stay well under
/// `u32::MAX`; saturate at the boundary instead of erroring so the UI can
/// still render a record if this ever happens.
fn ats_id_u32(id: u64) -> u32 {
    u32::try_from(id).unwrap_or(u32::MAX)
}

/// Build a `crate::domain::AtsRecord` out of a registry entry and its version map.
///
/// For each version, this loads the block it was created at (best effort —
/// older blocks may be outside the pinned window) to resolve timestamp,
/// extrinsic index, fee and signer.
pub async fn build_ats_record(
    api: &crate::data::rpc::client::AllfeatClient,
    at_tip: &OnlineClientAtBlock<SubstrateConfig>,
    ats_id: u64,
    ss58_prefix: u16,
) -> DataResult<Option<AtsRecord>> {
    let Some(record) = fetch_ats_registry_entry(at_tip, ats_id).await? else {
        return Ok(None);
    };
    let versions_raw = fetch_ats_versions(at_tip, ats_id).await?;

    let mut versions = Vec::with_capacity(versions_raw.len());
    let mut created_at_ms: i64 = 0;
    let created_at_block = versions_raw
        .first()
        .map(|(_, info)| info.created_at as u64)
        .unwrap_or(record.created_at as u64);

    for (v_idx, info) in versions_raw {
        let v_block = info.created_at as u64;
        let at_result = with_timeout("build_ats_record.at_block", async {
            api.at_block(v_block)
                .await
                .map_err(|e| DataError::Rpc(format!("at_block({v_block}): {e}")))
        })
        .await;
        let extras = match at_result {
            Ok(v_at) => {
                fetch_version_extras(&v_at, ats_id, v_idx, &record.owner, ss58_prefix).await?
            }
            Err(_) => VersionExtras {
                timestamp_ms: 0,
                extrinsic_index: 0,
                fee: 0,
                signer: encode_ss58(&record.owner.0, ss58_prefix),
            },
        };
        if v_idx == 0 {
            created_at_ms = extras.timestamp_ms;
        }
        versions.push(AtsVersion {
            version_index: v_idx,
            commitment: hex_bytes(&info.commitment),
            protocol_version: info.protocol_version,
            created_at_ms: extras.timestamp_ms,
            block_number: v_block,
            extrinsic_index: extras.extrinsic_index,
            extrinsic_id: extrinsic_id(v_block, extras.extrinsic_index),
            fee: extras.fee,
            signer: extras.signer,
        });
    }

    let deposits: Vec<Deposit> = record
        .deposits
        .0
        .iter()
        .map(|entry| Deposit {
            address: encode_ss58(&entry.depositor.0, ss58_prefix),
            amount: entry.amount,
        })
        .collect();
    let total_deposit: u128 = deposits.iter().map(|d| d.amount).sum();

    Ok(Some(AtsRecord {
        ats_id: ats_id_u32(ats_id),
        owner: encode_ss58(&record.owner.0, ss58_prefix),
        created_at_ms,
        created_at_block,
        version_count: record.version_count,
        deposits,
        total_deposit,
        versions,
    }))
}

/// List up to `count` ATS records, skipping the first `from_index` from the
/// newest-first ordering. Walks `AtsRegistry` descending from `NextAtsId - 1`,
/// silently skipping revoked slots.
///
/// Each `build_ats_record` call reads multiple storage keys + pins the
/// creator block per version, so the walk runs through a `buffered` stream:
/// `ATS_CONCURRENCY` records are in-flight at once, newest-first order is
/// preserved, and dropping the stream once `count` is filled cancels the
/// trailing in-flight tasks.
pub async fn build_ats_list(
    api: &crate::data::rpc::client::AllfeatClient,
    at_tip: &OnlineClientAtBlock<SubstrateConfig>,
    count: u32,
    from_index: u32,
    ss58_prefix: u16,
) -> DataResult<Vec<AtsRecord>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let next = fetch_next_ats_id(at_tip).await?;
    if next == 0 {
        return Ok(Vec::new());
    }

    let mut stream = fan_out_ats_records(api, at_tip, next, ss58_prefix);
    let mut out = Vec::with_capacity(count as usize);
    let mut skipped: u32 = 0;
    while let Some(rec) = stream.next().await.transpose()? {
        let Some(rec) = rec else {
            continue;
        };
        if skipped < from_index {
            skipped += 1;
            continue;
        }
        out.push(rec);
        if out.len() >= count as usize {
            break;
        }
    }
    Ok(out)
}

/// Build the newest-first parallel stream over live ATS records. Returns
/// `None` for revoked slots so callers can skip them while preserving the
/// natural registry order via `buffered` (not `buffer_unordered`).
fn fan_out_ats_records<'a>(
    api: &'a crate::data::rpc::client::AllfeatClient,
    at_tip: &'a OnlineClientAtBlock<SubstrateConfig>,
    next: u64,
    ss58_prefix: u16,
) -> impl futures::Stream<Item = DataResult<Option<AtsRecord>>> + 'a {
    // `next` is one past the last-allocated id. Iterate `[0, next)` in
    // reverse so the first item is the newest ATS.
    stream::iter((0..next).rev())
        .map(move |id| {
            let api = api.clone();
            let at_tip = at_tip.clone();
            async move { build_ats_record(&api, &at_tip, id, ss58_prefix).await }
        })
        .buffered(ATS_CONCURRENCY)
}

/// Flatten the ATS timeline into a `(ats, version)` feed newest-first. Walks
/// records descending through the parallel fan-out; for each, emits versions
/// latest-first.
pub async fn build_ats_feed(
    api: &crate::data::rpc::client::AllfeatClient,
    at_tip: &OnlineClientAtBlock<SubstrateConfig>,
    count: u32,
    from_index: u32,
    ss58_prefix: u16,
) -> DataResult<Vec<AtsFeedItem>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let next = fetch_next_ats_id(at_tip).await?;
    if next == 0 {
        return Ok(Vec::new());
    }

    let mut stream = fan_out_ats_records(api, at_tip, next, ss58_prefix);
    let mut out = Vec::with_capacity(count as usize);
    let mut skipped: u32 = 0;
    while let Some(rec) = stream.next().await.transpose()? {
        let Some(rec) = rec else {
            continue;
        };
        // Iterate versions newest → oldest.
        for v in rec.versions.iter().rev() {
            if skipped < from_index {
                skipped += 1;
                continue;
            }
            if out.len() >= count as usize {
                break;
            }
            out.push(AtsFeedItem {
                ats_id: rec.ats_id,
                owner: rec.owner.clone(),
                version_index: v.version_index,
                is_initial: v.version_index == 0,
                is_latest: v.version_index + 1 == rec.version_count,
                version_count: rec.version_count,
                commitment: v.commitment.clone(),
                protocol_version: v.protocol_version,
                block_number: v.block_number,
                extrinsic_id: v.extrinsic_id.clone(),
                timestamp_ms: v.created_at_ms,
                signer: v.signer.clone(),
            });
        }
        if out.len() >= count as usize {
            break;
        }
    }
    // Records are already ordered newest-first, and within each we walked
    // versions latest-first; a final time-based sort makes the ordering
    // stable when timestamps match the registry order (which they do by
    // construction).
    out.sort_by_key(|f| std::cmp::Reverse(f.timestamp_ms));
    Ok(out)
}

/// Number of ATS entries currently owned by `address`. Empty (including
/// malformed inputs) resolves to `0` — never an error.
pub async fn build_account_ats_count(
    at_tip: &OnlineClientAtBlock<SubstrateConfig>,
    address: &str,
) -> DataResult<u32> {
    let Some(id) = parse_ss58(address) else {
        return Ok(0);
    };
    let ids = fetch_owner_index(at_tip, &id).await?;
    Ok(ids.len() as u32)
}

/// All ATS records owned by `address`, newest-first, capped at `limit`.
pub async fn build_account_ats(
    api: &crate::data::rpc::client::AllfeatClient,
    at_tip: &OnlineClientAtBlock<SubstrateConfig>,
    address: &str,
    limit: u32,
    ss58_prefix: u16,
) -> DataResult<Vec<AtsRecord>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let Some(owner) = parse_ss58(address) else {
        return Ok(Vec::new());
    };
    let mut ids = fetch_owner_index(at_tip, &owner).await?;
    ids.sort_by(|a, b| b.cmp(a)); // newest first (higher id = more recent)
    ids.truncate(limit as usize);

    // Fan the per-id record builds out; `buffered` keeps newest-first order.
    let records: Vec<Option<AtsRecord>> = stream::iter(ids)
        .map(|id| {
            let api = api.clone();
            let at_tip = at_tip.clone();
            async move { build_ats_record(&api, &at_tip, id, ss58_prefix).await }
        })
        .buffered(ATS_CONCURRENCY)
        .try_collect()
        .await?;
    Ok(records.into_iter().flatten().collect())
}

/// Aggregate stats over the full registry. Walks every live ATS entry to
/// build totals — fine for dev chains, memoized in Phase 7.
pub async fn build_ats_stats(
    api: &crate::data::rpc::client::AllfeatClient,
    at_tip: &OnlineClientAtBlock<SubstrateConfig>,
    ss58_prefix: u16,
) -> DataResult<AtsStats> {
    let now_ms = fetch_timestamp(at_tip).await?;
    let next = fetch_next_ats_id(at_tip).await?;

    let mut total: u32 = 0;
    let mut total_versions: u32 = 0;
    let mut unique_owners = std::collections::HashSet::<String>::new();
    let mut total_deposited: u128 = 0;
    let mut multi_version_count: u32 = 0;
    let mut last_24h: u32 = 0;
    let mut last_7d: u32 = 0;
    let mut last_30d: u32 = 0;
    let mut first_registered_at_ms: i64 = 0;
    let mut genesis_block: u64 = 0;

    const H24: i64 = 24 * 60 * 60 * 1000;
    const D7: i64 = 7 * H24;
    const D30: i64 = 30 * H24;

    if next > 0 {
        // Walk the whole registry in parallel — stats are the hottest of the
        // ATS pages, and the aggregation below is pure-CPU so we can feed it
        // as fast as the buffered stream yields results.
        let mut stream = fan_out_ats_records(api, at_tip, next, ss58_prefix);
        while let Some(rec) = stream.next().await.transpose()? {
            let Some(rec) = rec else {
                continue;
            };
            total = total.saturating_add(1);
            total_versions = total_versions.saturating_add(rec.version_count);
            unique_owners.insert(rec.owner.clone());
            total_deposited = total_deposited.saturating_add(rec.total_deposit);
            if rec.version_count > 1 {
                multi_version_count = multi_version_count.saturating_add(1);
            }
            for v in &rec.versions {
                let age = now_ms - v.created_at_ms;
                if age < H24 {
                    last_24h += 1;
                }
                if age < D7 {
                    last_7d += 1;
                }
                if age < D30 {
                    last_30d += 1;
                }
            }
            // Track earliest creation — the smallest ats_id we've seen.
            if first_registered_at_ms == 0 || rec.created_at_ms < first_registered_at_ms {
                first_registered_at_ms = rec.created_at_ms;
                genesis_block = rec.created_at_block;
            }
        }
    }

    let multi_version_share = if total == 0 {
        0.0
    } else {
        multi_version_count as f32 / total as f32
    };

    Ok(AtsStats {
        total,
        total_versions,
        last_24h,
        last_7d,
        last_30d,
        unique_owners: unique_owners.len() as u32,
        avg_per_day: last_24h,
        protocol_version: 1,
        genesis_block,
        first_registered_at_ms,
        total_deposited,
        multi_version_share,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ats_id_u32_passes_through_small_ids() {
        assert_eq!(ats_id_u32(0), 0);
        assert_eq!(ats_id_u32(42), 42);
        assert_eq!(ats_id_u32(u32::MAX as u64), u32::MAX);
    }

    #[test]
    fn ats_id_u32_saturates_above_u32_max() {
        assert_eq!(ats_id_u32(u32::MAX as u64 + 1), u32::MAX);
        assert_eq!(ats_id_u32(u64::MAX), u32::MAX);
    }
}
