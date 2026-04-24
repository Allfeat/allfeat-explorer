//! Mock-backed [`ChainData`] implementation.
//!
//! Delegates to the synchronous generators in [`crate::mock::data`] and wraps
//! results in `Ok(...)` — the mock never fails. Exists so the rest of the
//! explorer can code against [`ChainData`] today; swapping to the subxt RPC
//! provider is a one-line change in [`crate::server::state`].
//!
//! ## Live streams
//!
//! `subscribe_*` is backed by one synthetic producer task per network.
//! Cadence is the spec's `block_time_secs`. On each tick the task advances a
//! virtual head from the wall clock, calls the deterministic generators for
//! the newly-minted block, and fans out to per-topic `broadcast` channels.
//! Subscribers only see items produced after they subscribed, so the
//! initial page state still comes from the `latest_*` methods — the stream
//! is strictly a tail.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::broadcast;
use tokio::time::{interval, MissedTickBehavior};
use tokio_stream::wrappers::BroadcastStream;

use crate::data::cursor::{
    AtsCursor, AtsFeedCursor, BlockCursor, EventCursor, EventPhaseKind, ExtrinsicCursor,
    TransferCursor,
};
use crate::data::filters::{
    block_matches_filters, event_matches_filters, extrinsic_matches_filters,
    transfer_matches_filters, AccountAtsFilters, AtsFeedFilters, AtsFilters, BlockFilters,
    EventFilters, ExtrinsicFilters, TransferFilters,
};
use crate::data::provider::{ensure_token_network, BoxStream, ChainData};
use crate::domain::{
    Account, Allocation, AtsFeedItem, AtsRecord, AtsStats, Block, BlockEvent, EnvelopeDetail,
    EnvelopeId, EventPhase, Extrinsic, Page, PageInfo, PageRequest, TokenOverview, Transfer,
};
use crate::mock::data as gen;
use crate::network::{ChainCtx, NetworkSpec};

use super::error::{DataError, DataResult};

/// Broadcast channel capacity per topic. Large enough that a temporarily
/// stalled subscriber (browser tab in the background) doesn't force a
/// producer skip at the cadences we run at (3–6s per block); small enough
/// that a run-away subscriber can't pin megabytes of blocks in memory.
const BROADCAST_CAPACITY: usize = 64;

struct NetworkStreams {
    blocks: broadcast::Sender<Block>,
    transfers: broadcast::Sender<Transfer>,
    ats_feed: broadcast::Sender<AtsFeedItem>,
}

pub struct MockProvider {
    /// Lazy per-network producer state. Populated on the first `subscribe_*`
    /// for a given network; afterwards every `Sender::subscribe()` reuses
    /// the same upstream. `std::sync::Mutex` is fine because all access is
    /// short and synchronous — we never hold the lock across an `await`.
    streams: Mutex<HashMap<&'static str, NetworkStreams>>,
}

impl MockProvider {
    pub fn new() -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
        }
    }

    /// Subscribe to a topic, lazily spawning the producer for the ctx's
    /// network on first use. Returns a `BroadcastStream`-backed boxed
    /// stream; lag notifications are swallowed with a trace so the stream
    /// keeps producing items instead of surfacing a `Result` to the UI.
    fn subscribe<T, F>(&self, spec: &'static NetworkSpec, pick: F) -> BoxStream<T>
    where
        T: Clone + Send + 'static,
        F: FnOnce(&NetworkStreams) -> broadcast::Receiver<T>,
    {
        let mut guard = self.streams.lock().expect("mock streams mutex poisoned");
        let entry = guard.entry(spec.id).or_insert_with(|| {
            let (blocks_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
            let (transfers_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
            let (ats_feed_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
            spawn_producer(
                spec,
                blocks_tx.clone(),
                transfers_tx.clone(),
                ats_feed_tx.clone(),
            );
            NetworkStreams {
                blocks: blocks_tx,
                transfers: transfers_tx,
                ats_feed: ats_feed_tx,
            }
        });
        let rx = pick(entry);
        box_broadcast_stream(rx)
    }
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChainData for MockProvider {
    async fn head_block(&self, ctx: ChainCtx) -> DataResult<u64> {
        Ok(ctx.head_block())
    }

    async fn block_by_number(&self, ctx: ChainCtx, number: u64) -> DataResult<Option<Block>> {
        if number > ctx.head_block() {
            return Ok(None);
        }
        Ok(Some(gen::build_block(ctx, number)))
    }

    async fn latest_blocks(
        &self,
        ctx: ChainCtx,
        req: PageRequest,
        filters: BlockFilters,
    ) -> DataResult<Page<Block>> {
        if req.count == 0 {
            return Ok(empty_block_page());
        }
        let head = ctx.head_block();
        // Cursor is exclusive; absent means "start at the tip". A cursor
        // above the head is clamped rather than returning an empty page —
        // a stale cursor should still show *something* coherent.
        let cursor = parse_block_cursor(&req)?;
        let tip = match cursor {
            Some(c) => c.block.saturating_sub(1).min(head),
            None => head,
        };

        // Fetch N+1 so `has_more` is free. The generator already
        // saturates on block 0, so requesting more than exist is safe.
        let probe = (req.count as u64).saturating_add(1) as u32;
        let mut items: Vec<Block> = gen::get_blocks(ctx, probe, tip)
            .into_iter()
            .filter(|b| block_matches_filters(b, &filters))
            .collect();
        let has_more = items.len() > req.count as usize;
        if has_more {
            items.truncate(req.count as usize);
        }
        let next_cursor = if has_more {
            items
                .last()
                .map(|b| BlockCursor { block: b.number }.to_string())
        } else {
            None
        };

        Ok(Page {
            items,
            page_info: PageInfo {
                total: Some(head + 1),
                next_cursor,
                has_more,
            },
        })
    }

    async fn extrinsics_in_block(&self, ctx: ChainCtx, block: u64) -> DataResult<Vec<Extrinsic>> {
        let b = gen::build_block(ctx, block);
        Ok(gen::build_extrinsics(ctx, block, b.extrinsic_count))
    }

    async fn events_in_block(&self, ctx: ChainCtx, block: u64) -> DataResult<Vec<BlockEvent>> {
        Ok(mock_events_for_block(ctx, block))
    }

    async fn latest_events(
        &self,
        ctx: ChainCtx,
        req: PageRequest,
        filters: EventFilters,
    ) -> DataResult<Page<BlockEvent>> {
        if req.count == 0 {
            return Ok(empty_event_page());
        }
        let cursor = parse_event_cursor(&req)?;
        let head = ctx.head_block();
        let scan_tip = match cursor {
            Some(c) => c.block.min(head),
            None => head,
        };
        let target = req.count.saturating_add(1) as usize;
        let mut items: Vec<BlockEvent> = Vec::with_capacity(target);

        // Walk backwards from the (possibly cursor-clamped) tip,
        // flattening each block's events newest-first. A fully-idle mock
        // chain just returns fewer rows; the `block == 0` guard
        // prevents an underflow on fresh test fixtures.
        let mut block = scan_tip;
        loop {
            let mut events = mock_events_for_block(ctx, block);
            events.reverse();
            for ev in events {
                if let Some(c) = cursor {
                    if (ev.block_number, ev.index) >= (c.block, c.index) {
                        continue;
                    }
                }
                if !event_matches_filters(&ev, &filters) {
                    continue;
                }
                if items.len() >= target {
                    break;
                }
                items.push(ev);
            }
            if items.len() >= target || block == 0 {
                break;
            }
            block -= 1;
        }

        let has_more = items.len() > req.count as usize;
        if has_more {
            items.truncate(req.count as usize);
        }
        let next_cursor = if has_more {
            items.last().map(|ev| {
                EventCursor {
                    block: ev.block_number,
                    phase: EventPhaseKind::from(ev.phase),
                    index: ev.index,
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

    async fn latest_extrinsics(
        &self,
        ctx: ChainCtx,
        req: PageRequest,
        filters: ExtrinsicFilters,
    ) -> DataResult<Page<Extrinsic>> {
        if req.count == 0 {
            return Ok(empty_extrinsic_page());
        }
        let cursor = parse_extrinsic_cursor(&req)?;
        // Generator walks back from the head until it fills the buffer.
        // Fetch N+1 and apply the cursor filter in memory — the generator
        // itself is cursor-agnostic, and the over-fetch stays bounded
        // because a busy chain fills the window in a handful of blocks.
        //
        // We ask for `count + margin` to leave room for cursor-discarded
        // rows within the block the cursor points at.
        let target = req.count.saturating_add(1) as usize;
        let raw = gen::get_latest_extrinsics(ctx, target.saturating_mul(2));
        let mut items: Vec<Extrinsic> = Vec::with_capacity(target);
        for ext in raw {
            if let Some(c) = cursor {
                if (ext.block_number, ext.index) >= (c.block, c.index) {
                    continue;
                }
            }
            if !extrinsic_matches_filters(&ext, &filters) {
                continue;
            }
            if items.len() >= target {
                break;
            }
            items.push(ext);
        }
        let has_more = items.len() > req.count as usize;
        if has_more {
            items.truncate(req.count as usize);
        }
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

    async fn extrinsic_by_id(&self, ctx: ChainCtx, id: &str) -> DataResult<Option<Extrinsic>> {
        let Some((block_str, idx_str)) = id.split_once('-') else {
            return Ok(None);
        };
        let Ok(block) = block_str.parse::<u64>() else {
            return Ok(None);
        };
        let Ok(idx) = idx_str.parse::<u32>() else {
            return Ok(None);
        };
        if block > ctx.head_block() {
            return Ok(None);
        }
        let b = gen::build_block(ctx, block);
        if idx >= b.extrinsic_count {
            return Ok(None);
        }
        let xs = gen::build_extrinsics(ctx, block, b.extrinsic_count);
        Ok(xs.into_iter().nth(idx as usize))
    }

    async fn latest_transfers(
        &self,
        ctx: ChainCtx,
        req: PageRequest,
        filters: TransferFilters,
    ) -> DataResult<Page<Transfer>> {
        if req.count == 0 {
            return Ok(empty_transfer_page());
        }
        let cursor = parse_transfer_cursor(&req)?;
        // Mock transfers don't carry a real `event_idx`; synthesize one
        // from the enclosing extrinsic index so the cursor grammar stays
        // round-trippable. Two transfers in the same block with the
        // same `extrinsic.index` are impossible on the mock surface
        // (it's one Balances.Transfer per signed balances extrinsic),
        // so this pseudo-idx is unique.
        let target = req.count.saturating_add(1) as usize;
        let raw = gen::get_transfers(ctx, target.saturating_mul(2));
        let mut items: Vec<Transfer> = Vec::with_capacity(target);
        for t in raw {
            let block = t.extrinsic.block_number;
            let event_idx = t.extrinsic.index;
            if let Some(c) = cursor {
                if (block, event_idx) >= (c.block, c.event_idx) {
                    continue;
                }
            }
            if !transfer_matches_filters(&t, &filters) {
                continue;
            }
            if items.len() >= target {
                break;
            }
            items.push(t);
        }
        let has_more = items.len() > req.count as usize;
        if has_more {
            items.truncate(req.count as usize);
        }
        let next_cursor = if has_more {
            items.last().map(|t| {
                TransferCursor {
                    block: t.extrinsic.block_number,
                    event_idx: t.extrinsic.index,
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

    async fn account_by_address(
        &self,
        ctx: ChainCtx,
        address: &str,
    ) -> DataResult<Option<Account>> {
        Ok(Some(gen::get_account(ctx, address)))
    }

    async fn top_accounts(&self, ctx: ChainCtx, count: u32) -> DataResult<Vec<Account>> {
        Ok(gen::get_top_accounts(ctx, count as usize))
    }

    async fn ats_stats(&self, ctx: ChainCtx) -> DataResult<AtsStats> {
        Ok(gen::get_ats_stats(ctx))
    }

    async fn ats_by_index(&self, ctx: ChainCtx, index: u32) -> DataResult<Option<AtsRecord>> {
        if index >= ctx.ats_total() {
            return Ok(None);
        }
        Ok(Some(gen::build_ats(ctx, index)))
    }

    async fn ats_list(
        &self,
        ctx: ChainCtx,
        req: PageRequest,
        _filters: AtsFilters,
    ) -> DataResult<Page<AtsRecord>> {
        if req.count == 0 {
            return Ok(empty_ats_record_page());
        }
        let cursor = parse_ats_cursor(&req)?;
        let total = ctx.ats_total();
        // Same cursor → from_index translation as the RPC provider:
        // newest-first position of id `X` is `(total - 1) - X`, so to
        // get rows with `id < cursor.id = X` we skip `total - X` items.
        let from_index = match cursor {
            Some(c) => total.saturating_sub(c.id),
            None => 0,
        };
        let probe = req.count.saturating_add(1);
        let raw = gen::get_ats_list(ctx, probe, from_index);
        let has_more = raw.len() > req.count as usize;
        let mut items = raw;
        if has_more {
            items.truncate(req.count as usize);
        }
        let next_cursor = if has_more {
            items.last().map(|r| AtsCursor { id: r.ats_id }.to_string())
        } else {
            None
        };
        Ok(Page {
            items,
            page_info: PageInfo {
                total: Some(total as u64),
                next_cursor,
                has_more,
            },
        })
    }

    async fn ats_version_feed(
        &self,
        ctx: ChainCtx,
        req: PageRequest,
        _filters: AtsFeedFilters,
    ) -> DataResult<Page<AtsFeedItem>> {
        if req.count == 0 {
            return Ok(empty_ats_feed_page());
        }
        let cursor = parse_ats_feed_cursor(&req)?;
        // Mock generator doesn't speak `(ats_id, version)` cursors,
        // so ask for a generous buffer and filter in-memory. Mock
        // registries are small (few hundred items), so this is
        // effectively free.
        let buffer = req
            .count
            .saturating_add(req.count)
            .max(64)
            .saturating_add(32);
        let raw = gen::get_ats_version_feed(ctx, buffer, 0);
        let mut filtered: Vec<AtsFeedItem> = raw
            .into_iter()
            .filter(|item| match cursor {
                Some(c) => (item.ats_id, item.version_index) < (c.ats_id, c.version),
                None => true,
            })
            .collect();
        let has_more = filtered.len() > req.count as usize;
        if has_more {
            filtered.truncate(req.count as usize);
        }
        let next_cursor = if has_more {
            filtered.last().map(|item| {
                AtsFeedCursor {
                    ats_id: item.ats_id,
                    version: item.version_index,
                }
                .to_string()
            })
        } else {
            None
        };
        Ok(Page {
            items: filtered,
            page_info: PageInfo {
                total: None,
                next_cursor,
                has_more,
            },
        })
    }

    async fn account_ats(
        &self,
        ctx: ChainCtx,
        address: &str,
        req: PageRequest,
        _filters: AccountAtsFilters,
    ) -> DataResult<Page<AtsRecord>> {
        if req.count == 0 {
            return Ok(empty_ats_record_page());
        }
        let cursor = parse_ats_cursor(&req)?;
        let total = gen::get_account_ats_count(ctx, address);
        // Fetch the whole owner set (small on the mock surface) and
        // filter by cursor in-memory.
        let raw = gen::get_account_ats(ctx, address, total);
        let mut filtered: Vec<AtsRecord> = raw
            .into_iter()
            .filter(|rec| match cursor {
                Some(c) => rec.ats_id < c.id,
                None => true,
            })
            .collect();
        let has_more = filtered.len() > req.count as usize;
        if has_more {
            filtered.truncate(req.count as usize);
        }
        let next_cursor = if has_more {
            filtered
                .last()
                .map(|r| AtsCursor { id: r.ats_id }.to_string())
        } else {
            None
        };
        Ok(Page {
            items: filtered,
            page_info: PageInfo {
                total: Some(total as u64),
                next_cursor,
                has_more,
            },
        })
    }

    async fn token_overview(&self, ctx: ChainCtx) -> DataResult<TokenOverview> {
        ensure_token_network(ctx, "token overview")?;
        Ok(gen::get_token_overview(ctx))
    }

    async fn envelope_detail(
        &self,
        ctx: ChainCtx,
        id: EnvelopeId,
    ) -> DataResult<Option<EnvelopeDetail>> {
        ensure_token_network(ctx, "envelope detail")?;
        Ok(Some(gen::get_envelope_detail(ctx, id)))
    }

    async fn account_allocations(
        &self,
        ctx: ChainCtx,
        address: &str,
    ) -> DataResult<Vec<Allocation>> {
        ensure_token_network(ctx, "account allocations")?;
        Ok(gen::get_account_allocations(ctx, address))
    }

    async fn runtime_details(
        &self,
        ctx: ChainCtx,
        at: Option<u64>,
    ) -> DataResult<crate::domain::RuntimeDetails> {
        // The mock surface has no runtime code or genesis state to read
        // — everything is synthesized from `NetworkSpec`. This path exists
        // so the design-iteration `mock` build renders every tile on the
        // runtime page without hitting the `—` placeholder; the values
        // are deterministic per network but deliberately fake (hash
        // prefix `0xf…d…e…`) so a reader skimming the page can tell
        // at a glance that it's a mock chain.
        let head = ctx.head_block();
        let at_block = at.unwrap_or(head).min(head);
        let spec = ctx.spec;
        let identity = crate::domain::RuntimeIdentity {
            spec_name: spec.spec_name.to_string(),
            impl_name: format!("{}-node", spec.spec_name),
            authoring_version: 1,
            spec_version: spec.spec_version,
            impl_version: 0,
            transaction_version: 25,
            state_version: Some(1),
        };
        Ok(crate::domain::RuntimeDetails {
            identity,
            code: crate::domain::RuntimeCodeInfo {
                hash: format!("0x{}", "fd".repeat(32)),
                size_bytes: 1_800_000,
                compressed: true,
            },
            genesis_hash: format!("0x{}", "ab".repeat(32)),
            at_block,
            at_block_hash: format!("0x{:064x}", at_block),
            metadata_version: crate::data::metadata::METADATA_VERSION,
        })
    }

    async fn runtime_upgrades(
        &self,
        ctx: ChainCtx,
    ) -> DataResult<Vec<crate::domain::RuntimeUpgrade>> {
        // Single-row "current only" — the mock doesn't model the
        // `system.CodeUpdated` event, so there's nothing historical to
        // report. Frames as "deployed at genesis": the mock's virtual
        // chain ran the same runtime from block 0, which matches what
        // the indexed path returns for a chain that's never upgraded.
        // `first_block = Some(0)` renders as the genesis label in the
        // UI and lights up the real deployment-block affordance.
        Ok(vec![crate::domain::RuntimeUpgrade {
            spec_version: ctx.spec.spec_version,
            first_block: Some(0),
            first_block_timestamp_ms: Some(ctx.spec.genesis_ms),
            is_current: true,
        }])
    }

    async fn subscribe_blocks(&self, ctx: ChainCtx) -> DataResult<BoxStream<Block>> {
        Ok(self.subscribe(ctx.spec, |s| s.blocks.subscribe()))
    }

    async fn subscribe_transfers(&self, ctx: ChainCtx) -> DataResult<BoxStream<Transfer>> {
        Ok(self.subscribe(ctx.spec, |s| s.transfers.subscribe()))
    }

    async fn subscribe_ats_feed(&self, ctx: ChainCtx) -> DataResult<BoxStream<AtsFeedItem>> {
        Ok(self.subscribe(ctx.spec, |s| s.ats_feed.subscribe()))
    }
}

fn parse_block_cursor(req: &PageRequest) -> DataResult<Option<BlockCursor>> {
    let Some(raw) = req.cursor.as_deref() else {
        return Ok(None);
    };
    raw.parse::<BlockCursor>()
        .map(Some)
        .map_err(|e| DataError::BadRequest(e.to_string()))
}

fn empty_block_page() -> Page<Block> {
    Page {
        items: Vec::new(),
        page_info: PageInfo::default(),
    }
}

fn parse_extrinsic_cursor(req: &PageRequest) -> DataResult<Option<ExtrinsicCursor>> {
    let Some(raw) = req.cursor.as_deref() else {
        return Ok(None);
    };
    raw.parse::<ExtrinsicCursor>()
        .map(Some)
        .map_err(|e| DataError::BadRequest(e.to_string()))
}

fn empty_extrinsic_page() -> Page<Extrinsic> {
    Page {
        items: Vec::new(),
        page_info: PageInfo::default(),
    }
}

fn parse_transfer_cursor(req: &PageRequest) -> DataResult<Option<TransferCursor>> {
    let Some(raw) = req.cursor.as_deref() else {
        return Ok(None);
    };
    raw.parse::<TransferCursor>()
        .map(Some)
        .map_err(|e| DataError::BadRequest(e.to_string()))
}

fn empty_transfer_page() -> Page<Transfer> {
    Page {
        items: Vec::new(),
        page_info: PageInfo::default(),
    }
}

fn parse_event_cursor(req: &PageRequest) -> DataResult<Option<EventCursor>> {
    let Some(raw) = req.cursor.as_deref() else {
        return Ok(None);
    };
    raw.parse::<EventCursor>()
        .map(Some)
        .map_err(|e| DataError::BadRequest(e.to_string()))
}

fn empty_event_page() -> Page<BlockEvent> {
    Page {
        items: Vec::new(),
        page_info: PageInfo::default(),
    }
}

fn parse_ats_cursor(req: &PageRequest) -> DataResult<Option<AtsCursor>> {
    let Some(raw) = req.cursor.as_deref() else {
        return Ok(None);
    };
    raw.parse::<AtsCursor>()
        .map(Some)
        .map_err(|e| DataError::BadRequest(e.to_string()))
}

fn parse_ats_feed_cursor(req: &PageRequest) -> DataResult<Option<AtsFeedCursor>> {
    let Some(raw) = req.cursor.as_deref() else {
        return Ok(None);
    };
    raw.parse::<AtsFeedCursor>()
        .map(Some)
        .map_err(|e| DataError::BadRequest(e.to_string()))
}

fn empty_ats_record_page() -> Page<AtsRecord> {
    Page {
        items: Vec::new(),
        page_info: PageInfo::default(),
    }
}

fn empty_ats_feed_page() -> Page<AtsFeedItem> {
    Page {
        items: Vec::new(),
        page_info: PageInfo::default(),
    }
}

/// Wrap a tokio `broadcast::Receiver` into a boxed `Stream` that skips lag
/// notifications. Subscribers that fall behind by more than the channel
/// capacity miss those items silently — better than bubbling the error up
/// through every layer when there's nothing the UI can do about it.
fn box_broadcast_stream<T: Clone + Send + 'static>(rx: broadcast::Receiver<T>) -> BoxStream<T> {
    Box::pin(BroadcastStream::new(rx).filter_map(|r| async move {
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::debug!(error = %e, "mock broadcast lag: dropping items");
                None
            }
        }
    }))
}

/// Producer task: ticks at `block_time_secs`, advances a virtual head from
/// the system clock, and publishes synthetic chain activity per block. Runs
/// for the lifetime of the process — the provider is held in `AppState`
/// which has Arc-scoped lifetime, so the task never needs to shut down.
fn spawn_producer(
    spec: &'static NetworkSpec,
    blocks_tx: broadcast::Sender<Block>,
    transfers_tx: broadcast::Sender<Transfer>,
    ats_feed_tx: broadcast::Sender<AtsFeedItem>,
) {
    let period = Duration::from_secs(spec.block_time_secs.max(1));
    tokio::spawn(async move {
        let mut ticker = interval(period);
        // Skip catch-up ticks if the runtime stalls: we only care about
        // emitting *new* blocks, not firing a burst after a pause.
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        let boot_ctx = ChainCtx::new(spec, system_now_ms());
        let mut last_head = boot_ctx.head_block();
        let mut last_ats_total = boot_ctx.ats_total();

        loop {
            ticker.tick().await;
            let ctx = ChainCtx::new(spec, system_now_ms());
            let head = ctx.head_block();
            if head <= last_head {
                // Wall clock hasn't advanced a whole block yet (e.g. a
                // short timer skew). Wait for the next tick.
                continue;
            }

            // Catch up any block gap. In practice this emits exactly one
            // block per tick, but if the runtime de-scheduled us we still
            // produce every block the chain would have seen. Each new block
            // arrives with `finalized: false` (delta == 0 in `build_block`);
            // re-emitting block `num - 2` here flips the store's existing
            // "in block" row to "finalized" as the head advances — the
            // frontend dedupes on block number, so the second push replaces
            // the first in place.
            for num in (last_head + 1)..=head {
                emit_block(ctx, num, &blocks_tx, &transfers_tx);
                if num >= 2 {
                    let _ = blocks_tx.send(gen::build_block(ctx, num - 2));
                }
            }
            last_head = head;

            // ATS feed: whenever `ats_total` advances, emit one feed item
            // per new ATS. Newest-first generator means the first slice is
            // the most recent — reverse so subscribers see chronological
            // order within a batch.
            let ats_now = ctx.ats_total();
            if ats_now > last_ats_total {
                let delta = ats_now - last_ats_total;
                let feed = gen::get_ats_version_feed(ctx, delta, 0);
                for item in feed.into_iter().rev() {
                    let _ = ats_feed_tx.send(item);
                }
                last_ats_total = ats_now;
            }
        }
    });
}

/// Build and publish one block's worth of synthetic activity. Splits block
/// meta from extrinsic extraction so the block can be `send`-moved without
/// cloning while we still keep the extrinsic count for transfer derivation.
fn emit_block(
    ctx: ChainCtx,
    num: u64,
    blocks_tx: &broadcast::Sender<Block>,
    transfers_tx: &broadcast::Sender<Transfer>,
) {
    let block = gen::build_block(ctx, num);
    let extrinsic_count = block.extrinsic_count;
    // Ignore send errors: they only mean no receivers are listening, which
    // is normal until the first WS client subscribes.
    let _ = blocks_tx.send(block);

    // Mirrors `gen::get_transfers`: scan `balances.*` signed extrinsics and
    // turn each into a `Transfer`. We request `extrinsic_count + 2` because
    // `build_extrinsics` may drop the trailing entries in weird cases and
    // we want at least every "real" balances call to show up. This is the
    // same over-ask the batch `get_transfers` uses.
    let extrinsics = gen::build_extrinsics(ctx, num, extrinsic_count + 2);
    for e in extrinsics.into_iter().filter(|e| e.module == "balances") {
        if let (Some(signer), Some((dest, amount))) =
            (e.signer.clone(), gen::balances_transfer_args(&e.args))
        {
            let _ = transfers_tx.send(Transfer {
                extrinsic: e,
                from: signer,
                to: dest,
                amount,
            });
        }
    }
}

fn system_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Mock has no Initialization/Finalization phase events, so every row here
/// is `ApplyExtrinsic`-scoped — the extrinsic-attached events are projected
/// 1:1 in their natural order so the block detail page and the dashboard
/// events feed stay consistent.
fn mock_events_for_block(ctx: ChainCtx, block: u64) -> Vec<BlockEvent> {
    let b = gen::build_block(ctx, block);
    let extrinsics = gen::build_extrinsics(ctx, block, b.extrinsic_count);
    let mut out = Vec::new();
    let mut idx: u32 = 0;
    for ext in extrinsics {
        for ev in ext.events {
            out.push(BlockEvent {
                block_number: block,
                index: idx,
                phase: EventPhase::ApplyExtrinsic { index: ext.index },
                module: ev.module,
                name: ev.name,
                fields: ev.fields,
                timestamp_ms: b.timestamp_ms,
            });
            idx += 1;
        }
    }
    out
}
