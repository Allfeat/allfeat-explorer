//! RPC-backed [`ChainData`] implementation.
//!
//! Routes each query to the right [`RpcClient`] based on `ctx.spec.id`. Every
//! call goes through a `moka::future::Cache` fronting the method (see
//! [`super::cache`]) so concurrent dashboard bursts coalesce into a single
//! RPC per key.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{stream, StreamExt, TryStreamExt};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

use crate::data::cursor::{
    AtsCursor, AtsFeedCursor, BlockCursor, EventCursor, EventPhaseKind, ExtrinsicCursor,
    TransferCursor,
};
use crate::data::error::{DataError, DataResult};
use crate::data::filters::{
    block_matches_filters, event_matches_filters, extrinsic_matches_filters,
    transfer_matches_filters, AccountAtsFilters, AtsFeedFilters, AtsFilters, BlockFilters,
    EventFilters, ExtrinsicFilters, TransferFilters,
};
use crate::data::provider::{ensure_token_network, BoxStream, ChainData};
use crate::domain::{
    Account, Allocation, AtsFeedItem, AtsRecord, AtsStats, Block, BlockEvent, EnvelopeDetail,
    EnvelopeId, Extrinsic, Page, PageInfo, PageRequest, RuntimeDetails, RuntimeUpgrade,
    TokenOverview, Transfer,
};
use crate::network::{ChainCtx, RuntimeKind};

use super::cache::cached;
use super::client::{race_timeout, SubxtClient, RpcClient};
use super::mappers;

/// Fan-out width for the parallel `at_block` scans that back the latest-*
/// methods. 8 is the sweet spot on a dev node: beyond that a single-threaded
/// node saturates and latency stops dropping; below it the wire sits idle
/// between decode passes. Calibrate on a prod node if the profile changes.
const FETCH_CONCURRENCY: usize = 8;

pub struct RpcProvider {
    /// Keyed by `NetworkSpec::id`. Built once at boot from
    /// [`crate::server::config::ServerConfig`] and immutable afterwards.
    clients: HashMap<&'static str, Arc<RpcClient>>,
}

impl RpcProvider {
    pub fn new(clients: HashMap<&'static str, Arc<RpcClient>>) -> Self {
        Self { clients }
    }

    fn client(&self, ctx: ChainCtx) -> DataResult<Arc<RpcClient>> {
        self.clients
            .get(ctx.spec.id)
            .cloned()
            .ok_or_else(|| DataError::NetworkUnconfigured(ctx.spec.id.to_string()))
    }
}

#[async_trait]
impl ChainData for RpcProvider {
    fn is_ready(&self) -> bool {
        // At least one upstream node has produced a finalized head — the
        // head subscription is running and we can serve coherent queries.
        // The mapping over `values` is cheap (handful of networks).
        self.clients.values().any(|c| c.finalized_head().is_some())
    }

    async fn head_block(&self, ctx: ChainCtx) -> DataResult<u64> {
        let client = self.client(ctx)?;
        best_head_number(&client).await
    }

    async fn block_by_number(&self, ctx: ChainCtx, number: u64) -> DataResult<Option<Block>> {
        let client = self.client(ctx)?;
        let ss58_prefix = client.ss58_prefix();
        let runtime_kind = client.runtime_kind();
        let finalized_head = finalized_head_number(&client).await?;
        let best_head = best_head_number(&client).await?;
        // Guard against numbers above the current chain tip so absurd queries
        // short-circuit without an RPC round-trip. Non-finalized blocks
        // (`finalized_head < number <= best_head`) are served here too —
        // the mapper marks them `finalized = false` and the node's pin
        // window still holds them for `at_block`.
        if number > best_head {
            return Ok(None);
        }
        // Finalized block bodies are immutable, so their entries can live in
        // the LRU forever. Non-finalized blocks ride the same cache because
        // an individual detail view is a one-shot read; if the user revisits
        // the same number after finality arrives, a fresh mapper call will
        // flip the `finalized` flag. Pinning fallout surfaces as a
        // `BlockNotFound` on the loader path — map that to `Ok(None)` at the
        // call site below so it stays out of the LRU and the next caller
        // re-tries against the fresher pin window.
        let cache = &client.caches().block_by_number;
        let client_for_load = client.clone();
        let hit = cached(cache, "block_by_number", number, async move {
            let api = client_for_load.subxt().await?;
            match race_timeout("at_block", api.at_block(number)).await? {
                Ok(at) => mappers::map_block(&at, finalized_head, runtime_kind, ss58_prefix).await,
                Err(e) if is_block_not_found(&e) => Err(DataError::InvalidPayload(format!(
                    "block {number} pin dropped"
                ))),
                Err(e) => Err(DataError::Rpc(format!("at_block({number}): {e}"))),
            }
        })
        .await;
        match hit {
            Ok(block) => Ok(Some(block)),
            Err(DataError::InvalidPayload(_)) => Ok(None),
            Err(e) => Err(e),
        }
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

        let cursor = parse_block_cursor(&req)?;
        let count = req.count;

        let client = self.client(ctx)?;
        let ss58_prefix = client.ss58_prefix();
        let runtime_kind = client.runtime_kind();
        let cache = &client.caches().latest_blocks;
        let client_for_load = client.clone();
        let best_head_hint = client.best_head();
        // Cache key includes the filter tuple so `?finalized=true` and
        // `?finalized=false` never collide. Keeping it a small tuple of
        // scalars keeps the cache-key comparison cheap — the hash-set
        // shape the cache uses doesn't need anything richer.
        let filter_key = (filters.finalized, filters.min_extrinsics);
        cached(
            cache,
            "latest_blocks",
            (count, cursor.map(|c| c.block), filter_key),
            async move {
                let api = client_for_load.subxt().await?;
                let finalized_head = finalized_head_number_uncached(&api).await?;
                // Best head may trail the first best-block notification on
                // a fresh connect; fall back to the finalized head, which
                // is a strict lower bound on the chain tip.
                let best_head = best_head_hint.unwrap_or(finalized_head);

                // No cursor → start at the chain tip, which includes the
                // one-or-two non-finalized blocks above the finalized head
                // so pending rows surface in the list. A cursor is
                // exclusive (next page starts strictly below it), and
                // anything absurd is clamped to the tip so a stale cursor
                // doesn't paint an empty page.
                let tip = match cursor {
                    Some(c) => c.block.saturating_sub(1).min(best_head),
                    None => best_head,
                };

                // Fetch N+1 so `has_more` is free: one extra block beyond
                // the caller's window tells us whether another page
                // exists without a separate `COUNT(*)` (which doesn't
                // even exist on the RPC path).
                let span = (count as u64).saturating_add(1);
                let lowest = tip.saturating_sub(span - 1);
                // Fan out the `at_block` + `map_block` pairs with
                // `buffered` so the window's worth of storage reads
                // pipeline on the WS instead of serializing at ~RTT
                // apiece. `buffered` (vs `buffer_unordered`) preserves
                // newest-first order. A `BlockNotFound` surfaces as
                // `None` and is filtered out: the UI can cope with a
                // short window, not a hole in the middle of one.
                let blocks: Vec<Option<Block>> = stream::iter((lowest..=tip).rev())
                    .map(|number| {
                        let api = api.clone();
                        async move {
                            match race_timeout("at_block", api.at_block(number)).await? {
                                Ok(at) => {
                                    mappers::map_block(&at, finalized_head, runtime_kind, ss58_prefix)
                                        .await
                                        .map(Some)
                                }
                                Err(e) if is_block_not_found(&e) => Ok(None),
                                Err(e) => Err(DataError::Rpc(format!("at_block({number}): {e}"))),
                            }
                        }
                    })
                    .buffered(FETCH_CONCURRENCY)
                    .try_collect()
                    .await?;
                // Filter before the has_more trim so the (count+1)-sized
                // window's overflow row is compared against the filtered
                // set, not the raw fetch. The filter can still under-
                // report `has_more` when the window scan missed matches
                // further back — the RPC path is a bounded fallback and
                // exact-total accuracy lives in the DB path.
                let mut items: Vec<Block> = blocks
                    .into_iter()
                    .flatten()
                    .filter(|b| block_matches_filters(b, &filters))
                    .collect();

                // Trim to the contract. The fetch-N+1 window may overshoot
                // past the request; any excess is the signal that another
                // page exists.
                let has_more = items.len() > count as usize;
                if has_more {
                    items.truncate(count as usize);
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
                        total: Some(best_head + 1),
                        next_cursor,
                        has_more,
                    },
                })
            },
        )
        .await
    }

    async fn extrinsics_in_block(&self, ctx: ChainCtx, block: u64) -> DataResult<Vec<Extrinsic>> {
        let client = self.client(ctx)?;
        let ss58_prefix = client.ss58_prefix();
        let runtime_kind = client.runtime_kind();
        let network_id = ctx.spec.id;
        if block > best_head_number(&client).await? {
            return Ok(Vec::new());
        }
        let cache = &client.caches().extrinsics_in_block;
        let client_for_load = client.clone();
        cached(cache, "extrinsics_in_block", block, async move {
            let api = client_for_load.subxt().await?;
            match race_timeout("at_block", api.at_block(block)).await? {
                Ok(at) => {
                    let timestamp_ms = mappers::fetch_timestamp(&at, runtime_kind).await?;
                    let events = mappers::fetch_block_events(&at).await?;
                    let events_by_phase = mappers::index_events_by_phase(&events)?;
                    mappers::map_extrinsics(
                        &at,
                        timestamp_ms,
                        &events_by_phase,
                        network_id,
                        runtime_kind,
                        ss58_prefix,
                    )
                    .await
                }
                Err(e) if is_block_not_found(&e) => Ok(Vec::new()),
                Err(e) => Err(DataError::Rpc(format!("at_block({block}): {e}"))),
            }
        })
        .await
    }

    async fn events_in_block(&self, ctx: ChainCtx, block: u64) -> DataResult<Vec<BlockEvent>> {
        let client = self.client(ctx)?;
        let ss58_prefix = client.ss58_prefix();
        let runtime_kind = client.runtime_kind();
        let network_id = ctx.spec.id;
        if block > best_head_number(&client).await? {
            return Ok(Vec::new());
        }
        let cache = &client.caches().events_in_block;
        let client_for_load = client.clone();
        cached(cache, "events_in_block", block, async move {
            let api = client_for_load.subxt().await?;
            match race_timeout("at_block", api.at_block(block)).await? {
                Ok(at) => {
                    let timestamp_ms = mappers::fetch_timestamp(&at, runtime_kind).await?;
                    let events = mappers::fetch_block_events(&at).await?;
                    mappers::map_block_events(block, timestamp_ms, &events, network_id, ss58_prefix)
                }
                Err(e) if is_block_not_found(&e) => Ok(Vec::new()),
                Err(e) => Err(DataError::Rpc(format!("at_block({block}): {e}"))),
            }
        })
        .await
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
        let count = req.count;

        let client = self.client(ctx)?;
        let ss58_prefix = client.ss58_prefix();
        let runtime_kind = client.runtime_kind();
        let network_id = ctx.spec.id;
        let cache = &client.caches().latest_events;
        let client_for_load = client.clone();
        let best_head_hint = client.best_head();
        let filter_key = (filters.pallet.clone(), filters.variant.clone());
        let filters_for_load = filters.clone();
        cached(
            cache,
            "latest_events",
            (count, cursor.map(|c| (c.block, c.index)), filter_key),
            async move {
                let api = client_for_load.subxt().await?;
                let finalized = finalized_head_number_uncached(&api).await?;
                let tip = best_head_hint.unwrap_or(finalized);
                let scan_tip = match cursor {
                    Some(c) => c.block.min(tip),
                    None => tip,
                };

                // Scan back from the tip, flattening each block's events in
                // reverse so the newest event of the newest block lands
                // first. Depth = count * 2 mirrors `latest_extrinsics`; a
                // busy chain fills the buffer in a handful of blocks.
                let target = (count as usize).saturating_add(1);
                let depth: u64 = (count as u64 * 2).max(count as u64);
                let lowest = scan_tip.saturating_sub(depth.saturating_sub(1));
                let mut stream = stream::iter((lowest..=scan_tip).rev())
                    .map(|number| {
                        let api = api.clone();
                        async move {
                            fetch_block_events_with_ts(&api, number, network_id, runtime_kind, ss58_prefix).await
                        }
                    })
                    .buffered(FETCH_CONCURRENCY);

                let mut items: Vec<BlockEvent> = Vec::with_capacity(target);
                'outer: while let Some(next) = stream.next().await {
                    let Some(mut events) = next? else { break };
                    // Reverse per-block so newest event of a block comes first.
                    events.reverse();
                    for ev in events {
                        if let Some(c) = cursor {
                            if (ev.block_number, ev.index) >= (c.block, c.index) {
                                continue;
                            }
                        }
                        if !event_matches_filters(&ev, &filters_for_load) {
                            continue;
                        }
                        if items.len() >= target {
                            break 'outer;
                        }
                        items.push(ev);
                    }
                }

                let has_more = items.len() > count as usize;
                if has_more {
                    items.truncate(count as usize);
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
            },
        )
        .await
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
        let count = req.count;

        let client = self.client(ctx)?;
        let ss58_prefix = client.ss58_prefix();
        let runtime_kind = client.runtime_kind();
        let network_id = ctx.spec.id;
        let cache = &client.caches().latest_extrinsics;
        let client_for_load = client.clone();
        let best_head_hint = client.best_head();
        // Cache key covers every filter field so different filter
        // combinations get their own slot. Cloning is cheap (these are
        // bounded strings) and keeps the async move below borrow-free.
        let filter_key = (
            filters.signed,
            filters.pallet.clone(),
            filters.call.clone(),
            filters.status,
        );
        let filters_for_load = filters.clone();
        cached(
            cache,
            "latest_extrinsics",
            (count, cursor.map(|c| (c.block, c.index)), filter_key),
            async move {
                let api = client_for_load.subxt().await?;
                let finalized = finalized_head_number_uncached(&api).await?;
                let best_head = best_head_hint.unwrap_or(finalized);
                // Start at the tip (or the block below the cursor, whichever
                // is smaller). A cursor past the tip is clamped — a stale
                // cursor should still paint something coherent.
                let scan_tip = match cursor {
                    Some(c) => c.block.min(best_head),
                    None => best_head,
                };

                // Fetch-N+1: try to produce `count + 1` rows so the caller
                // can tell from the result alone whether another page
                // exists. Depth is a heuristic — a sparse chain might run
                // out before we hit the cap, which is fine (`has_more` =
                // false then).
                let target = (count as usize).saturating_add(1);
                let depth: u64 = (count as u64 * 2).max(count as u64);
                let lowest = scan_tip.saturating_sub(depth.saturating_sub(1));
                let mut stream = stream::iter((lowest..=scan_tip).rev())
                    .map(|number| {
                        let api = api.clone();
                        async move {
                            fetch_block_extrinsics(&api, number, network_id, runtime_kind, ss58_prefix).await
                        }
                    })
                    .buffered(FETCH_CONCURRENCY);

                let mut items = Vec::with_capacity(target);
                'outer: while let Some(next) = stream.next().await {
                    let Some(extrinsics) = next? else { break };
                    for ext in extrinsics.into_iter().rev() {
                        if ext.index == 0 && !ext.signed {
                            continue;
                        }
                        // Cursor is exclusive: drop anything at-or-newer.
                        if let Some(c) = cursor {
                            if (ext.block_number, ext.index) >= (c.block, c.index) {
                                continue;
                            }
                        }
                        if !extrinsic_matches_filters(&ext, &filters_for_load) {
                            continue;
                        }
                        if items.len() >= target {
                            break 'outer;
                        }
                        items.push(ext);
                    }
                }

                let has_more = items.len() > count as usize;
                if has_more {
                    items.truncate(count as usize);
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
            },
        )
        .await
    }

    async fn extrinsic_by_id(&self, ctx: ChainCtx, id: &str) -> DataResult<Option<Extrinsic>> {
        let Some((block, idx)) = mappers::parse_extrinsic_id(id) else {
            return Ok(None);
        };
        let client = self.client(ctx)?;
        let ss58_prefix = client.ss58_prefix();
        let runtime_kind = client.runtime_kind();
        let network_id = ctx.spec.id;
        if block > best_head_number(&client).await? {
            return Ok(None);
        }
        let cache = &client.caches().extrinsic_by_id;
        let client_for_load = client.clone();
        // `try_get_with` doesn't cache a successful `None`, so we represent
        // "found" as `Some(Extrinsic)` in-cache and let misses bubble as an
        // `Ok(None)` from the loader (which just won't be stored — fine, they
        // only happen when the node drops the pin window).
        let hit = cached(cache, "extrinsic_by_id", (block, idx), async move {
            let api = client_for_load.subxt().await?;
            let at = match race_timeout("at_block", api.at_block(block)).await? {
                Ok(at) => at,
                Err(e) if is_block_not_found(&e) => {
                    return Err(DataError::Rpc(format!("block {block} pin dropped: {e}")));
                }
                Err(e) => return Err(DataError::Rpc(format!("at_block({block}): {e}"))),
            };
            let timestamp_ms = mappers::fetch_timestamp(&at, runtime_kind).await?;
            let events = mappers::fetch_block_events(&at).await?;
            let events_by_phase = mappers::index_events_by_phase(&events)?;
            let all = mappers::map_extrinsics(
                &at,
                timestamp_ms,
                &events_by_phase,
                network_id,
                runtime_kind,
                ss58_prefix,
            )
            .await?;
            all.into_iter()
                .find(|e| e.index == idx)
                .ok_or_else(|| DataError::InvalidPayload(format!("no extrinsic at index {idx}")))
        })
        .await;
        // Errors from the loader map back to "not found" for the UI — a
        // dropped pin or a bogus index is indistinguishable from a miss from
        // the caller's perspective. Log the detail for operators.
        match hit {
            Ok(ext) => Ok(Some(ext)),
            Err(e) => {
                tracing::debug!(?e, block, idx, "extrinsic_by_id miss");
                Ok(None)
            }
        }
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
        let count = req.count;

        let client = self.client(ctx)?;
        let ss58_prefix = client.ss58_prefix();
        let runtime_kind = client.runtime_kind();
        let network_id = ctx.spec.id;
        let cache = &client.caches().latest_transfers;
        let client_for_load = client.clone();
        let filter_key = (filters.from.clone(), filters.to.clone());
        let filters_for_load = filters.clone();
        cached(
            cache,
            "latest_transfers",
            (count, cursor.map(|c| (c.block, c.event_idx)), filter_key),
            async move {
                let api = client_for_load.subxt().await?;
                let tip = finalized_head_number_uncached(&api).await?;
                let scan_tip = match cursor {
                    Some(c) => c.block.min(tip),
                    None => tip,
                };

                // Cap the scan depth so a sparse dev chain doesn't walk
                // forever. A busy chain fills the buffer in a handful of
                // blocks; a quiet one simply returns what it has.
                const MAX_DEPTH: u64 = 200;
                let lowest = scan_tip.saturating_sub(MAX_DEPTH);

                let mut stream = stream::iter((lowest..=scan_tip).rev())
                    .map(|number| {
                        let api = api.clone();
                        async move {
                            fetch_block_transfers(&api, number, network_id, runtime_kind, ss58_prefix).await
                        }
                    })
                    .buffered(FETCH_CONCURRENCY);

                let target = (count as usize).saturating_add(1);
                let mut items: Vec<(Transfer, u32)> = Vec::with_capacity(target);
                'outer: while let Some(next) = stream.next().await {
                    let Some(transfers) = next? else { break };
                    // Newest-first within the block: reverse so event_idx
                    // descends, matching the DB path's
                    // `(block_num DESC, event_idx DESC)` ordering.
                    for (transfer, event_idx) in transfers.into_iter().rev() {
                        let block = transfer.extrinsic.block_number;
                        if let Some(c) = cursor {
                            if (block, event_idx) >= (c.block, c.event_idx) {
                                continue;
                            }
                        }
                        if !transfer_matches_filters(&transfer, &filters_for_load) {
                            continue;
                        }
                        if items.len() >= target {
                            break 'outer;
                        }
                        items.push((transfer, event_idx));
                    }
                }

                let has_more = items.len() > count as usize;
                if has_more {
                    items.truncate(count as usize);
                }
                let next_cursor = if has_more {
                    items.last().map(|(t, event_idx)| {
                        TransferCursor {
                            block: t.extrinsic.block_number,
                            event_idx: *event_idx,
                        }
                        .to_string()
                    })
                } else {
                    None
                };
                Ok(Page {
                    items: items.into_iter().map(|(t, _)| t).collect(),
                    page_info: PageInfo {
                        total: None,
                        next_cursor,
                        has_more,
                    },
                })
            },
        )
        .await
    }

    async fn account_by_address(
        &self,
        ctx: ChainCtx,
        address: &str,
    ) -> DataResult<Option<Account>> {
        let client = self.client(ctx)?;
        let ss58_prefix = client.ss58_prefix();
        let runtime_kind = client.runtime_kind();
        let cache = &client.caches().account_by_address;
        let addr_owned = address.to_string();
        let client_for_load = client.clone();
        cached(
            cache,
            "account_by_address",
            addr_owned.clone(),
            async move {
                let api = client_for_load.subxt().await?;
                let at = race_timeout("at_current_block", api.at_current_block())
                    .await?
                    .map_err(|e| DataError::Rpc(format!("at_current_block: {e}")))?;
                mappers::fetch_account(&at, &addr_owned, runtime_kind, ss58_prefix).await
            },
        )
        .await
    }

    async fn top_accounts(&self, ctx: ChainCtx, count: u32) -> DataResult<Vec<Account>> {
        let client = self.client(ctx)?;
        let ss58_prefix = client.ss58_prefix();
        let runtime_kind = client.runtime_kind();
        let cache = &client.caches().top_accounts;
        let client_for_load = client.clone();
        cached(cache, "top_accounts", count, async move {
            let api = client_for_load.subxt().await?;
            let at = race_timeout("at_current_block", api.at_current_block())
                .await?
                .map_err(|e| DataError::Rpc(format!("at_current_block: {e}")))?;
            mappers::fetch_top_accounts(&at, count, runtime_kind, ss58_prefix).await
        })
        .await
    }

    async fn ats_stats(&self, ctx: ChainCtx) -> DataResult<AtsStats> {
        let client = self.client(ctx)?;
        let ss58_prefix = client.ss58_prefix();
        let runtime_kind = client.runtime_kind();
        let network_id = ctx.spec.id;
        let cache = &client.caches().ats_stats;
        let client_for_load = client.clone();
        cached(cache, "ats_stats", (), async move {
            let api = client_for_load.subxt().await?;
            let at = race_timeout("at_current_block", api.at_current_block())
                .await?
                .map_err(|e| DataError::Rpc(format!("at_current_block: {e}")))?;
            mappers::build_ats_stats(&api, &at, network_id, runtime_kind, ss58_prefix).await
        })
        .await
    }

    async fn ats_by_id(&self, ctx: ChainCtx, ats_id: u32) -> DataResult<Option<AtsRecord>> {
        let client = self.client(ctx)?;
        let ss58_prefix = client.ss58_prefix();
        let runtime_kind = client.runtime_kind();
        let network_id = ctx.spec.id;
        let cache = &client.caches().ats_by_id;
        let client_for_load = client.clone();
        cached(cache, "ats_by_id", ats_id, async move {
            let api = client_for_load.subxt().await?;
            let at = race_timeout("at_current_block", api.at_current_block())
                .await?
                .map_err(|e| DataError::Rpc(format!("at_current_block: {e}")))?;
            mappers::build_ats_record(
                &api,
                &at,
                ats_id as u64,
                network_id,
                runtime_kind,
                ss58_prefix,
            )
            .await
        })
        .await
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
        let count = req.count;

        let client = self.client(ctx)?;
        let ss58_prefix = client.ss58_prefix();
        let runtime_kind = client.runtime_kind();
        let network_id = ctx.spec.id;
        let cache = &client.caches().ats_list;
        let client_for_load = client.clone();
        cached(
            cache,
            "ats_list",
            (count, cursor.map(|c| c.id)),
            async move {
                let api = client_for_load.subxt().await?;
                let at = race_timeout("at_current_block", api.at_current_block())
                    .await?
                    .map_err(|e| DataError::Rpc(format!("at_current_block: {e}")))?;
                let next = mappers::fetch_next_ats_id(&at, runtime_kind).await?;
                // Translate the cursor into the mapper's `from_index`
                // (position within the newest-first walk). Position of
                // `id` = `next - 1 - id`; to get rows with `id <
                // cursor.id = X` we start at position `next - X`.
                let from_index = match cursor {
                    Some(c) => next.saturating_sub(c.id as u64) as u32,
                    None => 0,
                };
                // Fetch-N+1 so `has_more` falls out of the response.
                let probe = count.saturating_add(1);
                let items = mappers::build_ats_list(
                    &api,
                    &at,
                    probe,
                    from_index,
                    network_id,
                    runtime_kind,
                    ss58_prefix,
                )
                .await?;
                let has_more = items.len() > count as usize;
                let mut trimmed = items;
                if has_more {
                    trimmed.truncate(count as usize);
                }
                let next_cursor = if has_more {
                    trimmed
                        .last()
                        .map(|rec| AtsCursor { id: rec.ats_id }.to_string())
                } else {
                    None
                };
                Ok(Page {
                    items: trimmed,
                    page_info: PageInfo {
                        // Total = one past the highest id (matches the DB
                        // path's `registry_total` — every id below `next`
                        // is at most revoked, not missing, for the MVP).
                        total: Some(next),
                        next_cursor,
                        has_more,
                    },
                })
            },
        )
        .await
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
        let count = req.count;

        let client = self.client(ctx)?;
        let ss58_prefix = client.ss58_prefix();
        let runtime_kind = client.runtime_kind();
        let network_id = ctx.spec.id;
        let cache = &client.caches().ats_version_feed;
        let client_for_load = client.clone();
        cached(
            cache,
            "ats_version_feed",
            (count, cursor.map(|c| (c.ats_id, c.version))),
            async move {
                let api = client_for_load.subxt().await?;
                let at = race_timeout("at_current_block", api.at_current_block())
                    .await?
                    .map_err(|e| DataError::Rpc(format!("at_current_block: {e}")))?;
                // The RPC feed walker doesn't know about `(ats_id,
                // version)` cursors natively, so we ask it for a
                // generous buffer and filter in-memory. This is fine
                // for the RPC fallback path — it's only ever hit on
                // non-indexed networks, which are dev chains with tiny
                // registries.
                let buffer = count.saturating_add(count).max(64);
                let raw = mappers::build_ats_feed(
                    &api,
                    &at,
                    buffer,
                    0,
                    network_id,
                    runtime_kind,
                    ss58_prefix,
                )
                .await?;
                let mut filtered: Vec<AtsFeedItem> = raw
                    .into_iter()
                    .filter(|item| match cursor {
                        Some(c) => (item.ats_id, item.version_index) < (c.ats_id, c.version),
                        None => true,
                    })
                    .collect();
                let has_more = filtered.len() > count as usize;
                if has_more {
                    filtered.truncate(count as usize);
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
            },
        )
        .await
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
        let count = req.count;
        let client = self.client(ctx)?;
        let ss58_prefix = client.ss58_prefix();
        let runtime_kind = client.runtime_kind();
        let network_id = ctx.spec.id;
        let cache = &client.caches().account_ats;
        let addr_owned = address.to_string();
        let client_for_load = client.clone();
        cached(
            cache,
            "account_ats",
            (addr_owned.clone(), count, cursor.map(|c| c.id)),
            async move {
                let api = client_for_load.subxt().await?;
                let at = race_timeout("at_current_block", api.at_current_block())
                    .await?
                    .map_err(|e| DataError::Rpc(format!("at_current_block: {e}")))?;
                // Count here is cheap (just the owner index length) —
                // use it both for the response `total` and to avoid
                // over-fetching when the window is narrow.
                let total =
                    mappers::build_account_ats_count(&at, &addr_owned, runtime_kind).await?;
                // Same cursor → limit translation as `ats_list`: fetch
                // the whole owner set (small by construction) and
                // filter in-memory. `build_account_ats` already returns
                // records newest-first.
                let raw = mappers::build_account_ats(
                    &api,
                    &at,
                    &addr_owned,
                    total,
                    network_id,
                    runtime_kind,
                    ss58_prefix,
                )
                .await?;
                let mut filtered: Vec<AtsRecord> = raw
                    .into_iter()
                    .filter(|rec| match cursor {
                        Some(c) => rec.ats_id < c.id,
                        None => true,
                    })
                    .collect();
                let has_more = filtered.len() > count as usize;
                if has_more {
                    filtered.truncate(count as usize);
                }
                let next_cursor = if has_more {
                    filtered
                        .last()
                        .map(|rec| AtsCursor { id: rec.ats_id }.to_string())
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
            },
        )
        .await
    }

    // Token allocation: mainnet-only pallet. The guard returns `NotSupported`
    // (→ 404 at the API boundary) for networks that don't ship it, mirroring
    // the mock path — callers on those networks would otherwise hit runtime
    // metadata mismatches deep inside the storage reads.

    async fn token_overview(&self, ctx: ChainCtx) -> DataResult<TokenOverview> {
        ensure_token_network(ctx, "token overview")?;
        let client = self.client(ctx)?;
        let ss58_prefix = client.ss58_prefix();
        let token_symbol = ctx.spec.token.to_string();
        let client_for_load = client.clone();
        cached(
            &client.caches().token_overview,
            "token_overview",
            (),
            async move {
                let api = client_for_load.subxt().await?;
                let at = race_timeout("at_current_block", api.at_current_block())
                    .await?
                    .map_err(|e| DataError::Rpc(format!("at_current_block: {e}")))?;
                let head = at.block_number();
                mappers::build_token_overview(&at, head, &token_symbol, ss58_prefix).await
            },
        )
        .await
    }

    async fn envelope_detail(
        &self,
        ctx: ChainCtx,
        id: EnvelopeId,
    ) -> DataResult<Option<EnvelopeDetail>> {
        ensure_token_network(ctx, "envelope detail")?;
        let client = self.client(ctx)?;
        let ss58_prefix = client.ss58_prefix();
        let client_for_load = client.clone();
        cached(
            &client.caches().envelope_detail,
            "envelope_detail",
            id,
            async move {
                let api = client_for_load.subxt().await?;
                let at = race_timeout("at_current_block", api.at_current_block())
                    .await?
                    .map_err(|e| DataError::Rpc(format!("at_current_block: {e}")))?;
                let head = at.block_number();
                mappers::build_envelope_detail(&at, id, head, ss58_prefix).await
            },
        )
        .await
    }

    async fn account_allocations(
        &self,
        ctx: ChainCtx,
        address: &str,
    ) -> DataResult<Vec<Allocation>> {
        ensure_token_network(ctx, "account allocations")?;
        let client = self.client(ctx)?;
        let ss58_prefix = client.ss58_prefix();
        let addr_owned = address.to_string();
        let client_for_load = client.clone();
        cached(
            &client.caches().account_allocations,
            "account_allocations",
            addr_owned.clone(),
            async move {
                let api = client_for_load.subxt().await?;
                let at = race_timeout("at_current_block", api.at_current_block())
                    .await?
                    .map_err(|e| DataError::Rpc(format!("at_current_block: {e}")))?;
                let head = at.block_number();
                mappers::build_account_allocations(&at, &addr_owned, head, ss58_prefix).await
            },
        )
        .await
    }

    async fn runtime_details(&self, ctx: ChainCtx, at: Option<u64>) -> DataResult<RuntimeDetails> {
        let client = self.client(ctx)?;
        // Resolve the "current" block number up front — the frontend
        // labels the hero with this value regardless of whether `at`
        // was supplied, and the at-block read still needs to be
        // clamped against the known chain tip to surface a clean 400
        // instead of a cryptic RPC error for future blocks.
        let best = best_head_number(&client).await?;
        let target = match at {
            Some(n) if n > best => {
                return Err(DataError::BadRequest(format!(
                    "at={n} is past the current head {best}"
                )));
            }
            Some(n) => Some(n),
            None => None,
        };
        // Three independent reads: they serialize on the shared
        // `OnlineClient` pool but the subxt backend already pipelines
        // WS frames, so running them back-to-back is as fast as
        // `try_join3` would be without the extra joiner-future
        // indirection. Any failure bubbles up unchanged — the handler
        // surfaces the error variant verbatim.
        let identity = client.runtime_identity(target).await?;
        let code = client.runtime_code_info(target).await?;
        let genesis_hash = client.genesis_hash().await?;
        // Pin the `at_block_hash` alongside the number so the UI can
        // render the exact hash the snapshot was taken at — matters
        // when the caller supplied `?at=N` and expects a stable tag.
        let at_block_number = target.unwrap_or(best);
        let api = client.subxt().await?;
        let at_ref = match target {
            Some(n) => race_timeout("at_block", api.at_block(n))
                .await?
                .map_err(|e| DataError::Rpc(format!("at_block({n}): {e}")))?,
            None => race_timeout("at_current_block", api.at_current_block())
                .await?
                .map_err(|e| DataError::Rpc(format!("at_current_block: {e}")))?,
        };
        let at_block_hash = mappers::hash_string(&at_ref.block_hash());
        Ok(RuntimeDetails {
            identity,
            code,
            genesis_hash,
            at_block: at_block_number,
            at_block_hash,
            metadata_version: crate::data::metadata::metadata_version_for(ctx.spec.id),
        })
    }

    async fn runtime_upgrades(&self, ctx: ChainCtx) -> DataResult<Vec<RuntimeUpgrade>> {
        // No historical `spec_version` index on the pure RPC path — we'd
        // have to walk `chain_getBlockHash(n) → Core_version` for every
        // block on the chain, which is prohibitive. Surface a single
        // "current only" entry so the UI still renders a coherent
        // timeline; the indexed provider overrides this with a real
        // SQL-derived history. `first_block = None` tells the frontend
        // "we don't know when this spec started, only that it's active
        // now" — and stays distinct from `Some(0)` (deployed at
        // genesis), which the indexed path may legitimately return.
        let client = self.client(ctx)?;
        let identity = client.runtime_identity(None).await?;
        Ok(vec![RuntimeUpgrade {
            spec_version: identity.spec_version,
            first_block: None,
            first_block_timestamp_ms: None,
            is_current: true,
        }])
    }

    async fn subscribe_blocks(&self, ctx: ChainCtx) -> DataResult<BoxStream<Block>> {
        let client = self.client(ctx)?;
        // Force the connection + watch task to be running before handing
        // back a receiver. Without this, a fresh process that never served
        // an HTTP query would hand back a silent stream until the first
        // RPC request happened to spawn `stream_blocks`.
        client.subxt().await?;
        Ok(box_broadcast(client.subscribe_blocks()))
    }

    async fn subscribe_transfers(&self, ctx: ChainCtx) -> DataResult<BoxStream<Transfer>> {
        let client = self.client(ctx)?;
        client.subxt().await?;
        Ok(box_broadcast(client.subscribe_transfers()))
    }

    async fn subscribe_ats_feed(&self, ctx: ChainCtx) -> DataResult<BoxStream<AtsFeedItem>> {
        let client = self.client(ctx)?;
        client.subxt().await?;
        Ok(box_broadcast(client.subscribe_ats_feed()))
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

/// Wrap a `tokio::broadcast::Receiver` into the trait-object stream shape.
/// Lag notifications are dropped with a trace: surfacing them to the UI
/// layer isn't actionable, and the subscriber stays alive for the next
/// item.
fn box_broadcast<T: Clone + Send + 'static>(rx: broadcast::Receiver<T>) -> BoxStream<T> {
    Box::pin(BroadcastStream::new(rx).filter_map(|r| async move {
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::debug!(error = %e, "rpc broadcast lag: dropping items");
                None
            }
        }
    }))
}

/// Number of the latest finalized block. Three layers, cheapest first:
///
/// 1. The live [`RpcClient::finalized_head`] watch, fed by the background
///    subscription task. This is an in-memory read — no RPC — whenever the
///    subscription has produced at least one notification.
/// 2. The hot moka cache (`finalized_head`), same one the older provider
///    code relied on, so a fresh connect that hasn't yet received a
///    subscription update still coalesces concurrent requests.
/// 3. A direct `at_current_block` call, loaded through the cache.
async fn finalized_head_number(client: &Arc<RpcClient>) -> DataResult<u64> {
    if let Some(head) = client.finalized_head() {
        return Ok(head);
    }
    let cache = &client.caches().finalized_head;
    let client_for_load = client.clone();
    cached(cache, "finalized_head", (), async move {
        let api = client_for_load.subxt().await?;
        finalized_head_number_uncached(&api).await
    })
    .await
}

/// Direct lookup, used inside cache loaders that already sit behind a
/// `cached(...)` call and would otherwise re-enter the same entry.
async fn finalized_head_number_uncached(api: &SubxtClient) -> DataResult<u64> {
    let at = race_timeout("at_current_block", api.at_current_block())
        .await?
        .map_err(|e| DataError::Rpc(format!("fetch finalized head: {e}")))?;
    Ok(at.block_number())
}

/// Best (possibly non-finalized) head. Prefers the live
/// [`RpcClient::best_head`] watch when the best-block subscription has
/// published at least once; falls back to the finalized head otherwise,
/// which is a strict lower bound on the chain tip (every finalized block
/// is also a best block). Used as the "tip" for list pagination and the
/// guard ceiling for block-by-number lookups so non-finalized blocks are
/// reachable instead of hidden behind a 404.
async fn best_head_number(client: &Arc<RpcClient>) -> DataResult<u64> {
    if let Some(best) = client.best_head() {
        return Ok(best);
    }
    finalized_head_number(client).await
}

/// Distinguish "block doesn't exist yet" from genuine RPC failures so callers
/// can return `None` instead of bubbling an error to the page.
fn is_block_not_found(err: &subxt::error::OnlineClientAtBlockError) -> bool {
    use subxt::error::OnlineClientAtBlockError as E;
    matches!(err, E::BlockNotFound { .. } | E::BlockHeaderNotFound { .. })
}

/// Pin a single block and map its extrinsic list. Returns `Ok(None)` for
/// `BlockNotFound` so the caller can treat a dropped pin window the same as
/// the end of the chain — the scan just stops there.
async fn fetch_block_extrinsics(
    api: &SubxtClient,
    number: u64,
    network_id: &str,
    runtime_kind: RuntimeKind,
    ss58_prefix: u16,
) -> DataResult<Option<Vec<Extrinsic>>> {
    match race_timeout("at_block", api.at_block(number)).await? {
        Ok(at) => {
            let timestamp_ms = mappers::fetch_timestamp(&at, runtime_kind).await?;
            let events = mappers::fetch_block_events(&at).await?;
            let events_by_phase = mappers::index_events_by_phase(&events)?;
            let extrinsics = mappers::map_extrinsics(
                &at,
                timestamp_ms,
                &events_by_phase,
                network_id,
                runtime_kind,
                ss58_prefix,
            )
            .await?;
            Ok(Some(extrinsics))
        }
        Err(e) if is_block_not_found(&e) => Ok(None),
        Err(e) => Err(DataError::Rpc(format!("at_block({number}): {e}"))),
    }
}

/// Pin a single block and decode its full event set (every phase). Returns
/// `Ok(None)` for `BlockNotFound` so the caller can treat a dropped pin
/// the same as the end of the chain.
async fn fetch_block_events_with_ts(
    api: &SubxtClient,
    number: u64,
    network_id: &str,
    runtime_kind: RuntimeKind,
    ss58_prefix: u16,
) -> DataResult<Option<Vec<BlockEvent>>> {
    match race_timeout("at_block", api.at_block(number)).await? {
        Ok(at) => {
            let timestamp_ms = mappers::fetch_timestamp(&at, runtime_kind).await?;
            let events = mappers::fetch_block_events(&at).await?;
            Ok(Some(mappers::map_block_events(
                number,
                timestamp_ms,
                &events,
                network_id,
                ss58_prefix,
            )?))
        }
        Err(e) if is_block_not_found(&e) => Ok(None),
        Err(e) => Err(DataError::Rpc(format!("at_block({number}): {e}"))),
    }
}

/// Pin a single block and map its transfer list together with each
/// transfer's `event_idx` inside the block. The extra tuple is what the
/// paginated read path needs to synthesize a [`TransferCursor`]; the
/// live subscription path uses [`mappers::map_transfers`] instead.
async fn fetch_block_transfers(
    api: &SubxtClient,
    number: u64,
    network_id: &str,
    runtime_kind: RuntimeKind,
    ss58_prefix: u16,
) -> DataResult<Option<Vec<(Transfer, u32)>>> {
    match race_timeout("at_block", api.at_block(number)).await? {
        Ok(at) => {
            let timestamp_ms = mappers::fetch_timestamp(&at, runtime_kind).await?;
            let events = mappers::fetch_block_events(&at).await?;
            let events_by_phase = mappers::index_events_by_phase(&events)?;
            let extrinsics = mappers::map_extrinsics(
                &at,
                timestamp_ms,
                &events_by_phase,
                network_id,
                runtime_kind,
                ss58_prefix,
            )
            .await?;
            let transfers = mappers::map_transfers_with_event_idx(
                &extrinsics,
                &events_by_phase,
                runtime_kind,
                ss58_prefix,
            )?;
            Ok(Some(transfers))
        }
        Err(e) if is_block_not_found(&e) => Ok(None),
        Err(e) => Err(DataError::Rpc(format!("at_block({number}): {e}"))),
    }
}
