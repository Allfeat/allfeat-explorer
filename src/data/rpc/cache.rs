//! Per-network caches layered in front of the subxt client.
//!
//! Two flavours:
//!
//! * **Hot** (`TTL = HOT_TTL_SECS`, cap [`HOT_CAPACITY`]) for head-following
//!   queries — latest blocks, ATS stats, anything whose answer mutates every
//!   block. TTL sits below the fastest network's block time so a cached
//!   entry is at most one block stale.
//!
//! * **Finalized** (LRU, cap [`FINALIZED_CAPACITY`], no TTL) for answers
//!   keyed on `(network, block_number)` where `block_number ≤ finalized_head`.
//!   Safe to pin forever because finalized blocks don't reorg.
//!
//! [`moka::future::Cache::try_get_with`] coalesces concurrent misses for the
//! same key (singleflight): 100 concurrent dashboard renders still result in
//! one RPC. Errors are propagated as `Arc<DataError>` by moka and cloned back
//! into `DataError` at the call site (cloneable since [`super::super::error`]).
//!
//! Cache miss paths emit `tracing::trace!` so hit-rate can be observed in
//! logs without bloating hits with per-request spans.
//!
//! One instance per [`super::client::RpcClient`]; keys are scoped by the
//! enclosing network so no network-id prefixing is needed here.

use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache;

use crate::data::error::{DataError, DataResult};
use crate::domain::{
    Account, Allocation, AtsFeedItem, AtsRecord, AtsStats, Block, BlockEvent, CallResult,
    EnvelopeDetail, EnvelopeId, Extrinsic, Page, TokenOverview, Transfer,
};

/// Per-listing cache-key tuples. Kept as aliases so the `Caches` struct
/// stays readable and the call sites in `provider.rs` don't have to
/// repeat the filter shape every time a field is added.
///
/// Each key packs `(count, cursor, filter-scalars)`. The filter scalars
/// mirror the ordering in [`crate::data::filters`] so a reader can
/// reconcile the two files at a glance.
pub type LatestBlocksKey = (u32, Option<u64>, (Option<bool>, Option<u32>));
pub type LatestExtrinsicsKey = (
    u32,
    Option<(u64, u32)>,
    (
        Option<bool>,
        Option<String>,
        Option<String>,
        Option<CallResult>,
    ),
);
pub type LatestTransfersKey = (u32, Option<(u64, u32)>, (Option<String>, Option<String>));
pub type LatestEventsKey = (u32, Option<(u64, u32)>, (Option<String>, Option<String>));
/// `(count, (ats_id, version)-cursor)` key for the ATS version feed.
pub type AtsVersionFeedKey = (u32, Option<(u32, u32)>);

/// Staleness budget for hot entries, in seconds. Kept below the fastest
/// configured block time (Melodie: 3s) so a cached head is at most one block
/// behind.
pub const HOT_TTL_SECS: u64 = 2;

/// Upper bound for any hot cache. Small because entries expire quickly.
pub const HOT_CAPACITY: u64 = 1_024;

/// Upper bound for any finalized LRU. Sized for a couple of days of historical
/// browsing at expected query rates while keeping RSS predictable.
pub const FINALIZED_CAPACITY: u64 = 50_000;

/// Grouped moka caches attached to a single [`super::client::RpcClient`].
///
/// All fields are cheap to clone (`Cache` is `Arc`-internal), but each
/// `RpcClient` owns exactly one instance and hands out `&Caches` — callers
/// don't need to clone.
pub struct Caches {
    // ── Head / finalized tracking ──────────────────────────────────────────
    /// Single-entry hot cache for the latest finalized head number. Each
    /// `ChainData` call resolves the head before deciding whether the
    /// finalized LRU is safe to consult; memoising it under TTL coalesces
    /// the head lookup across a dashboard burst.
    pub finalized_head: Cache<(), u64>,

    // ── Blocks ─────────────────────────────────────────────────────────────
    /// `number → Block` for blocks at or below the finalized head.
    /// Immutable once finalized, so no TTL.
    pub block_by_number: Cache<u64, Block>,
    /// `(count, cursor, filters) → Page<Block>` for the latest-blocks
    /// window. The filter tuple lives in the key so `?finalized=true`
    /// and `?finalized=false` never share a slot — an alias would make
    /// a filtered request serve unfiltered rows (or vice versa) from
    /// the cache.
    pub latest_blocks: Cache<LatestBlocksKey, Page<Block>>,

    // ── Extrinsics ─────────────────────────────────────────────────────────
    /// `block_number → Vec<Extrinsic>` for finalized blocks. The list is
    /// stable because both the block body and the event set it decodes from
    /// are frozen once finalized.
    pub extrinsics_in_block: Cache<u64, Vec<Extrinsic>>,
    /// `block_number → Vec<BlockEvent>` for finalized blocks. Same
    /// finality-stable rationale as `extrinsics_in_block`.
    pub events_in_block: Cache<u64, Vec<BlockEvent>>,
    /// `(count, cursor, filters) → Page<Extrinsic>` for the paginated
    /// latest-extrinsics list. Filters are part of the key for the
    /// same reason as `latest_blocks` — two requests with different
    /// filter values must not collide.
    pub latest_extrinsics: Cache<LatestExtrinsicsKey, Page<Extrinsic>>,
    /// `(block, idx) → Extrinsic` — finalized-only, keyed on the parsed id
    /// so non-normalised ids (e.g. "123-04") map to the same slot.
    pub extrinsic_by_id: Cache<(u64, u32), Extrinsic>,

    // ── Transfers ──────────────────────────────────────────────────────────
    /// `(count, cursor, filters) → Page<Transfer>` for the paginated
    /// transfer feed. `from` / `to` participate in the key.
    pub latest_transfers: Cache<LatestTransfersKey, Page<Transfer>>,

    /// `(count, cursor, filters) → Page<BlockEvent>` for the paginated
    /// chain-wide event feed. Cursor is `(block, event_idx)` (the
    /// phase letter is decorative and not part of the ordering key);
    /// filters are `(pallet, variant)`.
    pub latest_events: Cache<LatestEventsKey, Page<BlockEvent>>,

    // ── Accounts ───────────────────────────────────────────────────────────
    /// Balances and nonces follow the head of the chain — always hot.
    pub account_by_address: Cache<String, Option<Account>>,
    pub top_accounts: Cache<u32, Vec<Account>>,

    // ── ATS ────────────────────────────────────────────────────────────────
    /// Single-entry stats snapshot. Walks the whole registry today — the
    /// hot cache keeps the cost amortised over the TTL window.
    pub ats_stats: Cache<(), AtsStats>,
    /// Keyed by chain `ats_id`. The record itself can mutate (new
    /// versions, revocation) so this lives in the hot tier even though
    /// the id is stable.
    pub ats_by_id: Cache<u32, Option<AtsRecord>>,
    /// `(count, cursor) → Page<AtsRecord>` for the paginated registry
    /// list. Cursor is the newest-first ATS id.
    pub ats_list: Cache<(u32, Option<u32>), Page<AtsRecord>>,
    /// `(count, cursor) → Page<AtsFeedItem>` for the version feed.
    /// Cursor is `(ats_id, version)` lex.
    pub ats_version_feed: Cache<AtsVersionFeedKey, Page<AtsFeedItem>>,
    /// `(address, count, cursor) → Page<AtsRecord>` for the
    /// owner-scoped ATS list.
    pub account_ats: Cache<(String, u32, Option<u32>), Page<AtsRecord>>,

    // ── Token allocation (mainnet-only) ────────────────────────────────────
    /// Single-entry overview: envelopes + treasury + epoch snapshot. Walks
    /// the full allocations map to aggregate, so the TTL coalesces a
    /// dashboard burst into one walk.
    pub token_overview: Cache<(), TokenOverview>,
    /// Per-envelope detail. `Option` because an envelope may not be
    /// registered yet; the miss is cached so 404s don't repeatedly iterate.
    pub envelope_detail: Cache<EnvelopeId, Option<EnvelopeDetail>>,
    pub account_allocations: Cache<String, Vec<Allocation>>,
}

impl Caches {
    pub fn new() -> Self {
        Self {
            finalized_head: hot_cache(1),

            block_by_number: finalized_cache(),
            latest_blocks: hot_cache(HOT_CAPACITY),

            extrinsics_in_block: finalized_cache(),
            events_in_block: finalized_cache(),
            latest_extrinsics: hot_cache(HOT_CAPACITY),
            extrinsic_by_id: finalized_cache(),

            latest_transfers: hot_cache(HOT_CAPACITY),

            latest_events: hot_cache(HOT_CAPACITY),

            account_by_address: hot_cache(HOT_CAPACITY),
            top_accounts: hot_cache(HOT_CAPACITY),

            ats_stats: hot_cache(1),
            ats_by_id: hot_cache(HOT_CAPACITY),
            ats_list: hot_cache(HOT_CAPACITY),
            ats_version_feed: hot_cache(HOT_CAPACITY),
            account_ats: hot_cache(HOT_CAPACITY),

            token_overview: hot_cache(1),
            envelope_detail: hot_cache(HOT_CAPACITY),
            account_allocations: hot_cache(HOT_CAPACITY),
        }
    }
}

impl Default for Caches {
    fn default() -> Self {
        Self::new()
    }
}

fn hot_cache<K, V>(capacity: u64) -> Cache<K, V>
where
    K: std::hash::Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    Cache::builder()
        .max_capacity(capacity)
        .time_to_live(Duration::from_secs(HOT_TTL_SECS))
        .build()
}

fn finalized_cache<K, V>() -> Cache<K, V>
where
    K: std::hash::Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    Cache::builder().max_capacity(FINALIZED_CAPACITY).build()
}

/// Wrap a fallible loader in a `try_get_with` call, unwrapping the
/// `Arc<DataError>` that moka returns on coalesced failures so callers keep
/// the flat `DataResult` surface. Emits a miss-trace before running the
/// loader — hits stay silent so the log volume tracks the RPC volume.
pub async fn cached<K, V, F>(
    cache: &Cache<K, V>,
    label: &'static str,
    key: K,
    load: F,
) -> DataResult<V>
where
    K: std::fmt::Debug + std::hash::Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    F: std::future::Future<Output = DataResult<V>> + Send + 'static,
{
    let key_for_trace = key.clone();
    cache
        .try_get_with(key, async move {
            tracing::trace!(cache = label, key = ?key_for_trace, "rpc cache miss");
            load.await
        })
        .await
        .map_err(|arc: Arc<DataError>| (*arc).clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    /// Canonical singleflight check: 100 concurrent misses on the same key
    /// must dispatch the loader exactly once. This is the `moka` contract
    /// that every `cached(...)` call in `provider.rs` relies on — pin it so
    /// a future `moka` upgrade can't silently regress it.
    #[tokio::test]
    async fn cached_coalesces_concurrent_misses_to_one_load() {
        let cache: Cache<u64, u64> = Cache::builder().max_capacity(16).build();
        let counter = Arc::new(AtomicU64::new(0));

        let mut handles = Vec::with_capacity(100);
        for _ in 0..100 {
            let cache = cache.clone();
            let counter = counter.clone();
            handles.push(tokio::spawn(async move {
                let counter = counter.clone();
                cached(&cache, "test", 0u64, async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    // Sleep long enough that the other 99 tasks reach
                    // `try_get_with` while this one is still loading.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok::<u64, DataError>(42)
                })
                .await
            }));
        }

        for h in handles {
            assert_eq!(h.await.unwrap().unwrap(), 42);
        }
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "100 concurrent misses must map to exactly 1 loader execution",
        );
    }

    /// Failures from the loader must surface as the original `DataError`
    /// (cloned out of the `Arc` moka wraps them in), not as a new, lossy
    /// `Rpc(<Arc<DataError> as Display>)` string. Otherwise every retry
    /// would lose the original variant and the caller can't classify it.
    #[tokio::test]
    async fn cached_propagates_concrete_data_error_variant() {
        let cache: Cache<u64, u64> = Cache::builder().max_capacity(4).build();
        let err = cached(&cache, "test", 1u64, async {
            Err::<u64, _>(DataError::decode("boom"))
        })
        .await
        .expect_err("loader failed");
        assert!(
            matches!(err, DataError::Decode(ref s) if s == "boom"),
            "expected `Decode(\"boom\")`, got {err:?}",
        );
    }

    /// A second call on the same key must be essentially free (<1ms) — that's
    /// the whole point of the finalized LRU layer. Relative check instead of
    /// wall-clock so the test stays deterministic on slow CI.
    #[tokio::test]
    async fn cached_hit_is_faster_than_miss() {
        let cache: Cache<u64, u64> = Cache::builder().max_capacity(4).build();

        let t0 = Instant::now();
        cached(&cache, "test", 7u64, async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Ok::<u64, DataError>(7)
        })
        .await
        .unwrap();
        let miss = t0.elapsed();

        let t1 = Instant::now();
        cached(&cache, "test", 7u64, async {
            // Would trigger a 20ms sleep again if hit went through — the
            // singleflight path should short-circuit before this runs.
            tokio::time::sleep(Duration::from_millis(20)).await;
            Ok::<u64, DataError>(7)
        })
        .await
        .unwrap();
        let hit = t1.elapsed();

        assert!(
            hit < miss / 4,
            "cache hit ({hit:?}) should be at least 4× faster than the miss ({miss:?})",
        );
    }
}
