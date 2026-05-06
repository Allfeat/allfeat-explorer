//! [`ChainData`] implementation backed by Postgres with an RPC fallback.
//!
//! Multi-tenant: one deployment indexes several networks into the same
//! DB. Each indexed network has its own pending buffer (tip state)
//! keyed by `network_id`. Queries for a non-indexed network flow
//! through to the RPC fallback.
//!
//! The routing lives **inside** the provider — no separate
//! `HybridProvider` trait — so pages never have to know which backend
//! served a given query.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use futures::StreamExt;
use sqlx::PgPool;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

use crate::data::cursor::{BlockCursor, ExtrinsicCursor};
use crate::data::error::{DataError, DataResult};
use crate::data::filters::{
    extrinsic_matches_filters, AccountAtsFilters, AtsFeedFilters, AtsFilters, BlockFilters,
    EventFilters, ExtrinsicFilters, TransferFilters,
};
use crate::data::provider::{BoxStream, ChainData};
use crate::data::rpc::RpcProvider;
use crate::domain::{
    Account, Allocation, AtsFeedItem, AtsRecord, AtsStats, Block, BlockEvent, EnvelopeDetail,
    EnvelopeId, Extrinsic, Page, PageInfo, PageRequest, RuntimeDetails, RuntimeUpgrade,
    TokenOverview, Transfer,
};
use crate::indexer::buffer::SharedBuffer;
use crate::indexer::lookups::NetworkLookup;
use crate::network::ChainCtx;

use super::queries;
use super::queries::extrinsics::ExtrinsicLookup;

/// Hybrid-but-opaque provider: DB-first for the indexed networks, RPC
/// fallback for everything else.
pub struct IndexedProvider {
    pool: PgPool,
    /// Set of network ids this deployment indexes. Queries for any
    /// other network flow straight through to the RPC fallback.
    indexed: HashSet<&'static str>,
    /// Tip buffers, one per indexed network. Empty when there's no
    /// buffer wired at all (`--mode=indexer` writer-only deployments);
    /// a missing entry for a specific network means the DB is the only
    /// source for that network's reads.
    buffers: HashMap<&'static str, SharedBuffer>,
    /// RPC path used for non-indexed networks and for methods that
    /// aren't served from the DB (live subscriptions, ATS feed
    /// streams). `Arc` so the same underlying clients can be shared
    /// with the indexer workers.
    rpc: Arc<RpcProvider>,
    /// Shared handle to the `&str → SMALLINT` network id mapping.
    /// Populated by [`crate::server::AppState::seed_lookups`] after
    /// boot-time migrations run. Readers `get()` the populated value
    /// per call — missing lookup surfaces as a clean
    /// [`DataError::Rpc`] with "network lookup not initialised"
    /// instead of a mysterious sqlx type-mismatch.
    network_lookup: Arc<OnceLock<Arc<NetworkLookup>>>,
}

impl IndexedProvider {
    /// Build a provider that indexes `indexed_ids` into `pool` and
    /// falls back to `rpc` for anything else.
    pub fn new(
        pool: PgPool,
        indexed_ids: impl IntoIterator<Item = &'static str>,
        rpc: Arc<RpcProvider>,
        network_lookup: Arc<OnceLock<Arc<NetworkLookup>>>,
    ) -> Self {
        Self {
            pool,
            indexed: indexed_ids.into_iter().collect(),
            buffers: HashMap::new(),
            rpc,
            network_lookup,
        }
    }

    /// Attach a shared pending buffer for `network_id`. Networks
    /// without a buffer serve everything straight from Postgres.
    pub fn with_buffer(mut self, network_id: &'static str, buffer: SharedBuffer) -> Self {
        self.buffers.insert(network_id, buffer);
        self
    }

    fn is_indexed(&self, ctx: ChainCtx) -> bool {
        self.indexed.contains(ctx.spec.id)
    }

    fn buffer_for(&self, ctx: ChainCtx) -> Option<&SharedBuffer> {
        self.buffers.get(ctx.spec.id)
    }

    /// Resolve `ctx.spec.id` → `i16` for the `network_id` SMALLINT column.
    /// Surfaces a clean error when the lookup hasn't been seeded yet
    /// (boot sequence bug) or when the network isn't registered.
    fn network_sid(&self, ctx: ChainCtx) -> DataResult<i16> {
        let lookup = self.network_lookup.get().ok_or_else(|| {
            DataError::Rpc(format!(
                "network lookup not initialised (indexed path requires seed_lookups before use): {}",
                ctx.spec.id
            ))
        })?;
        lookup.resolve(ctx.spec.id).ok_or_else(|| {
            DataError::Rpc(format!(
                "network id {:?} not registered in `networks` table",
                ctx.spec.id
            ))
        })
    }

    /// SS58 prefix to use when rendering addresses for `ctx`. Reads the
    /// hardcoded value from [`crate::network::NetworkSpec::ss58_prefix`] —
    /// we don't fetch `ss58Format` from the node because the value is
    /// effectively immutable per chain.
    fn ss58_prefix(&self, ctx: ChainCtx) -> u16 {
        ctx.spec.ss58_prefix
    }
}

#[async_trait]
impl ChainData for IndexedProvider {
    fn is_ready(&self) -> bool {
        // Readiness is still driven by "at least one upstream node has
        // produced a finalized head" — the pool being up doesn't help
        // if the chain isn't talking to us. The status endpoint
        // exposes the indexer-specific lag separately.
        self.rpc.is_ready()
    }

    async fn head_block(&self, ctx: ChainCtx) -> DataResult<u64> {
        // Head is always the chain's finalized tip, not the indexer's
        // cursor — otherwise pagination bounds shrink whenever the
        // indexer falls behind and the UI silently refuses to show
        // blocks that do exist on chain. The banner calls out the lag.
        self.rpc.head_block(ctx).await
    }

    async fn block_by_number(&self, ctx: ChainCtx, number: u64) -> DataResult<Option<Block>> {
        if !self.is_indexed(ctx) {
            return self.rpc.block_by_number(ctx, number).await;
        }
        let ss58_prefix = self.ss58_prefix(ctx);
        let sid = self.network_sid(ctx)?;
        if let Some(block) =
            queries::blocks::block_by_number(&self.pool, sid, number, ss58_prefix).await?
        {
            return Ok(Some(block));
        }
        // Block not in the DB yet — either the indexer is still catching up
        // to this finalized number or the number sits above the finalized
        // head (a pending block). The RPC path serves both.
        self.rpc.block_by_number(ctx, number).await
    }

    async fn latest_blocks(
        &self,
        ctx: ChainCtx,
        req: PageRequest,
        filters: BlockFilters,
    ) -> DataResult<Page<Block>> {
        if !self.is_indexed(ctx) {
            return self.rpc.latest_blocks(ctx, req, filters).await;
        }
        if req.count == 0 {
            return Ok(Page::empty());
        }
        let ss58_prefix = self.ss58_prefix(ctx);

        // Parse the cursor once here so both the pending slice and the DB
        // query share the same upper bound. An unparseable cursor is a
        // 400, not a silent reset to page 1 — callers whose cursor format
        // drifts need to know.
        let cursor = parse_block_cursor(&req)?;

        // `total` is the chain head + 1, not the indexer head: users
        // expect the total to match what the chain thinks exists, even
        // if the indexer is lagging. The banner calls out the lag
        // separately.
        let sid = self.network_sid(ctx)?;
        let chain_head = self.head_block(ctx).await.ok();
        let indexed_head = queries::blocks::indexed_head(&self.pool, sid)
            .await?
            .unwrap_or(0);

        // Pending slice: blocks above `indexed_head` that the DB doesn't yet
        // own. Only needed when the requested window can reach that zone —
        // with a cursor that already sits at or below `indexed_head`,
        // everything flows through the DB.
        let pending_upper = match cursor {
            Some(c) => c.block.saturating_sub(1),
            None => chain_head.unwrap_or(indexed_head),
        };

        let mut pending: Vec<Block> = Vec::new();
        // `filters.finalized == Some(true)` skips the pending slice
        // entirely: pending rows are by definition non-finalized and
        // would just be filtered out. The RPC call is the expensive
        // part; skipping saves a round-trip.
        if pending_upper > indexed_head && filters.finalized != Some(true) {
            // Delegate the pending window to the RPC path with a
            // same-shape request so it rides its own cache + timeouts.
            // The RPC path applies the filters itself, so anything we
            // receive here already passed them.
            let pending_req = PageRequest {
                count: req.count,
                cursor: cursor.map(|c| c.to_string()),
            };
            let rpc_page = self.rpc.latest_blocks(ctx, pending_req, filters).await?;
            for b in rpc_page.items {
                if b.number > indexed_head {
                    pending.push(b);
                    if pending.len() >= req.count as usize {
                        break;
                    }
                } else {
                    // The RPC window already spilled into finalized
                    // territory; stop early so the DB path owns those
                    // rows (authoritative + cheaper).
                    break;
                }
            }
        }

        let pending_len = pending.len();
        let mut items: Vec<Block> = pending;

        // Work out how many more blocks we still need and what the DB
        // upper bound should be. The cursor (if any) and the
        // `indexed_head` both cap the DB window.
        let need = req.count as usize - items.len();
        let mut has_more = false;
        let mut next_cursor: Option<String> = None;

        if need > 0 {
            let probe = (need + 1) as u32;
            let db_upper = indexed_head.min(cursor.map_or(u64::MAX, |c| c.block.saturating_sub(1)));
            let db_page = queries::blocks::list_blocks_page_bounded(
                &self.pool,
                sid,
                probe,
                // No explicit cursor here — the db query already capped
                // at `db_upper`, which itself encodes the cursor.
                None,
                Some(db_upper),
                &filters,
                ss58_prefix,
            )
            .await?;

            // The DB helper treats `probe` as its own `count` and pops the
            // extra row into `has_more`. Re-derive the actual `has_more`
            // relative to the outer request's `need`: if the DB returned
            // exactly the (need+1)-sized probe, we have more; otherwise
            // its own `has_more` flag already reflects the truth.
            if db_page.items.len() > need {
                // Too many rows — pop to exactly `need` and mark has_more.
                let mut trimmed = db_page.items;
                trimmed.truncate(need);
                if let Some(last) = trimmed.last() {
                    next_cursor = Some(BlockCursor { block: last.number }.to_string());
                }
                has_more = true;
                items.extend(trimmed);
            } else {
                items.extend(db_page.items);
                has_more = db_page.page_info.has_more;
                next_cursor = db_page.page_info.next_cursor;
            }
        } else if pending_len == req.count as usize {
            // Filled the page purely from pending blocks. Only more data
            // exists when there are still finalized rows below the
            // pending slice.
            if indexed_head > 0 && pending_upper > indexed_head {
                has_more = true;
                if let Some(last) = items.last() {
                    next_cursor = Some(BlockCursor { block: last.number }.to_string());
                }
            }
        }

        Ok(Page {
            items,
            page_info: PageInfo {
                total: chain_head.map(|h| h + 1),
                next_cursor,
                has_more,
            },
        })
    }

    async fn extrinsics_in_block(&self, ctx: ChainCtx, block: u64) -> DataResult<Vec<Extrinsic>> {
        if !self.is_indexed(ctx) {
            return self.rpc.extrinsics_in_block(ctx, block).await;
        }
        if let Some(buffer) = self.buffer_for(ctx) {
            let buf = buffer.read().await;
            if block > buf.finalized_head() {
                if let Some(buffered) = buf.iter_blocks_newest_first().find(|b| b.number == block) {
                    return Ok(buffered.extrinsics.clone());
                }
            }
        }
        let ss58_prefix = self.ss58_prefix(ctx);
        let sid = self.network_sid(ctx)?;
        let db_rows = queries::extrinsics::extrinsics_in_block(
            &self.pool,
            ctx.spec.id,
            sid,
            block,
            ss58_prefix,
        )
        .await?;
        if !db_rows.is_empty() {
            return Ok(db_rows);
        }
        // Empty from the DB could be either "no extrinsics in this block"
        // (very rare — every block has the timestamp inherent) or "this
        // block isn't indexed yet". Disambiguate by checking whether the
        // block number is above the indexer tip; if so, fall through to
        // the RPC path so pending blocks' extrinsics are reachable.
        let indexed_head = queries::blocks::indexed_head(&self.pool, sid)
            .await?
            .unwrap_or(0);
        if block > indexed_head {
            return self.rpc.extrinsics_in_block(ctx, block).await;
        }
        Ok(db_rows)
    }

    async fn events_in_block(&self, ctx: ChainCtx, block: u64) -> DataResult<Vec<BlockEvent>> {
        // Events aren't indexed in Postgres yet — route every request through
        // the RPC path so the block detail page still sees every phase.
        self.rpc.events_in_block(ctx, block).await
    }

    async fn latest_events(
        &self,
        ctx: ChainCtx,
        req: PageRequest,
        filters: EventFilters,
    ) -> DataResult<Page<BlockEvent>> {
        // Same reason as `events_in_block`: no Postgres index for events,
        // so the RPC path is authoritative. The recent-block window is
        // tiny, so the extra subxt hops are cheap.
        self.rpc.latest_events(ctx, req, filters).await
    }

    async fn latest_extrinsics(
        &self,
        ctx: ChainCtx,
        req: PageRequest,
        filters: ExtrinsicFilters,
    ) -> DataResult<Page<Extrinsic>> {
        if !self.is_indexed(ctx) {
            return self.rpc.latest_extrinsics(ctx, req, filters).await;
        }
        if req.count == 0 {
            return Ok(Page::empty());
        }

        let cursor = parse_extrinsic_cursor(&req)?;
        let ss58_prefix = self.ss58_prefix(ctx);
        let sid = self.network_sid(ctx)?;

        // Pending slice: extrinsics from the buffered (non-finalized) tip
        // that the DB doesn't yet own. The buffer stores blocks newest-
        // first; we walk them in the same order so the concatenation with
        // the DB slice stays monotonically descending on `(block, idx)`.
        // Filters are applied here too so the pending rows we keep match
        // what the DB query below would have returned for the same window.
        let mut items: Vec<Extrinsic> = Vec::with_capacity(req.count as usize);
        let mut last_buffered: Option<(u64, u32)> = None;
        if let Some(buffer) = self.buffer_for(ctx) {
            let buf = buffer.read().await;
            'outer: for b in buf.iter_blocks_newest_first() {
                for ext in b.extrinsics.iter().rev() {
                    if ext.index == 0 && !ext.signed {
                        continue;
                    }
                    // Skip anything past the caller-supplied cursor — the
                    // cursor is exclusive, so rows at-or-newer-than it
                    // belong to the previous page.
                    if let Some(c) = cursor {
                        if (ext.block_number, ext.index) >= (c.block, c.index) {
                            continue;
                        }
                    }
                    // Track `last_buffered` even for filtered-out rows so
                    // the DB upper bound still walks past everything the
                    // buffer already covered — skipping it would let the
                    // DB hand back rows the buffer owns.
                    last_buffered = Some((ext.block_number, ext.index));
                    if !extrinsic_matches_filters(ext, &filters) {
                        continue;
                    }
                    if items.len() >= req.count as usize {
                        break 'outer;
                    }
                    items.push(ext.clone());
                }
            }
        }

        // Did we fill the page entirely from the buffer? Decide `has_more`
        // by probing for one more row in the DB below the last buffered
        // entry (or below the cursor, if no buffered rows were taken).
        if items.len() >= req.count as usize {
            let probe_upper = last_buffered.or_else(|| cursor.map(|c| (c.block, c.index)));
            let probe_req = PageRequest {
                count: 1,
                cursor: None,
            };
            let probe = queries::extrinsics::list_extrinsics_page_bounded(
                &self.pool,
                ctx.spec.id,
                sid,
                &probe_req,
                probe_upper,
                &filters,
                ss58_prefix,
            )
            .await?;
            let has_more = !probe.items.is_empty();
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
            return Ok(Page {
                items,
                page_info: PageInfo {
                    total: None,
                    next_cursor,
                    has_more,
                },
            });
        }

        // Otherwise top up from the DB. The upper bound there is the last
        // buffered row (exclusive) so the DB never hands back a row the
        // buffer already returned.
        let need = req.count - items.len() as u32;
        let db_req = PageRequest {
            count: need,
            cursor: None,
        };
        let db_upper = last_buffered.or_else(|| cursor.map(|c| (c.block, c.index)));
        let db_page = queries::extrinsics::list_extrinsics_page_bounded(
            &self.pool,
            ctx.spec.id,
            sid,
            &db_req,
            db_upper,
            &filters,
            ss58_prefix,
        )
        .await?;

        let has_more = db_page.page_info.has_more;
        let mut next_cursor = db_page.page_info.next_cursor;
        items.extend(db_page.items);
        if has_more && next_cursor.is_none() {
            // Defensive: `db_page` set `has_more` but no cursor. Fall back to
            // the last item we kept so the caller can still paginate.
            next_cursor = items.last().map(|e| {
                ExtrinsicCursor {
                    block: e.block_number,
                    index: e.index,
                }
                .to_string()
            });
        }

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
        if !self.is_indexed(ctx) {
            return self.rpc.extrinsic_by_id(ctx, id).await;
        }
        let Some(lookup) = queries::extrinsics::parse_lookup(id) else {
            return Ok(None);
        };

        if let Some(buffer) = self.buffer_for(ctx) {
            let buf = buffer.read().await;
            match &lookup {
                ExtrinsicLookup::BlockIdx { block, idx } => {
                    if let Some(ext) = buf.extrinsic_by_block_idx(*block, *idx) {
                        return Ok(Some(ext));
                    }
                }
                ExtrinsicLookup::Hash(h) => {
                    if let Some(ext) = buf.extrinsic_by_hash(h) {
                        return Ok(Some(ext));
                    }
                }
            }
        }

        let ss58_prefix = self.ss58_prefix(ctx);
        let sid = self.network_sid(ctx)?;
        match lookup {
            ExtrinsicLookup::BlockIdx { block, idx } => {
                if let Some(ext) = queries::extrinsics::extrinsic_by_block_idx(
                    &self.pool,
                    ctx.spec.id,
                    sid,
                    block,
                    idx,
                    ss58_prefix,
                )
                .await?
                {
                    return Ok(Some(ext));
                }
                // Fall through to RPC for extrinsics in blocks above the
                // indexer tip (pending or indexer-lag) — the RPC provider
                // parses the same `block-idx` id format and pins the
                // block itself.
                let indexed_head = queries::blocks::indexed_head(&self.pool, sid)
                    .await?
                    .unwrap_or(0);
                if block > indexed_head {
                    return self.rpc.extrinsic_by_id(ctx, id).await;
                }
                Ok(None)
            }
            ExtrinsicLookup::Hash(h) => {
                queries::extrinsics::extrinsic_by_hash(
                    &self.pool,
                    ctx.spec.id,
                    sid,
                    &h,
                    ss58_prefix,
                )
                .await
            }
        }
    }

    async fn latest_transfers(
        &self,
        ctx: ChainCtx,
        req: PageRequest,
        filters: TransferFilters,
    ) -> DataResult<Page<Transfer>> {
        if !self.is_indexed(ctx) {
            return self.rpc.latest_transfers(ctx, req, filters).await;
        }
        // Skip the pending buffer for paginated reads: buffered transfers
        // don't carry the `event_idx` the cursor grammar requires, and
        // the tip-of-chain window is already served live via the
        // WebSocket `subscribe_transfers` topic. The indexer's steady-
        // state lag is small, so the list-view omission is a fraction of
        // a block window in the worst case.
        let ss58_prefix = self.ss58_prefix(ctx);
        let sid = self.network_sid(ctx)?;
        queries::transfers::list_transfers_page(
            &self.pool,
            ctx.spec.id,
            sid,
            &req,
            &filters,
            ss58_prefix,
        )
        .await
    }

    async fn account_by_address(
        &self,
        ctx: ChainCtx,
        address: &str,
    ) -> DataResult<Option<Account>> {
        if !self.is_indexed(ctx) {
            return self.rpc.account_by_address(ctx, address).await;
        }
        let ss58_prefix = self.ss58_prefix(ctx);
        let sid = self.network_sid(ctx)?;
        queries::accounts::account_by_address(&self.pool, sid, address, ss58_prefix).await
    }

    async fn top_accounts(&self, ctx: ChainCtx, count: u32) -> DataResult<Vec<Account>> {
        if !self.is_indexed(ctx) {
            return self.rpc.top_accounts(ctx, count).await;
        }
        let ss58_prefix = self.ss58_prefix(ctx);
        let sid = self.network_sid(ctx)?;
        queries::accounts::top_accounts(&self.pool, sid, count, ss58_prefix).await
    }

    async fn ats_stats(&self, ctx: ChainCtx) -> DataResult<AtsStats> {
        if !self.is_indexed(ctx) {
            return self.rpc.ats_stats(ctx).await;
        }
        let sid = self.network_sid(ctx)?;
        queries::ats::ats_stats(&self.pool, sid).await
    }

    async fn ats_by_id(&self, ctx: ChainCtx, ats_id: u32) -> DataResult<Option<AtsRecord>> {
        if !self.is_indexed(ctx) {
            return self.rpc.ats_by_id(ctx, ats_id).await;
        }
        let ss58_prefix = self.ss58_prefix(ctx);
        let sid = self.network_sid(ctx)?;
        queries::ats::ats_by_id(&self.pool, sid, ats_id, ss58_prefix).await
    }

    async fn ats_list(
        &self,
        ctx: ChainCtx,
        req: PageRequest,
        filters: AtsFilters,
    ) -> DataResult<Page<AtsRecord>> {
        if !self.is_indexed(ctx) {
            return self.rpc.ats_list(ctx, req, filters).await;
        }
        let ss58_prefix = self.ss58_prefix(ctx);
        let sid = self.network_sid(ctx)?;
        queries::ats::list_ats_page(&self.pool, sid, &req, &filters, ss58_prefix).await
    }

    async fn ats_version_feed(
        &self,
        ctx: ChainCtx,
        req: PageRequest,
        filters: AtsFeedFilters,
    ) -> DataResult<Page<AtsFeedItem>> {
        if !self.is_indexed(ctx) {
            return self.rpc.ats_version_feed(ctx, req, filters).await;
        }
        let ss58_prefix = self.ss58_prefix(ctx);
        let sid = self.network_sid(ctx)?;
        queries::ats::list_ats_feed_page(&self.pool, sid, &req, &filters, ss58_prefix).await
    }

    async fn account_ats(
        &self,
        ctx: ChainCtx,
        address: &str,
        req: PageRequest,
        filters: AccountAtsFilters,
    ) -> DataResult<Page<AtsRecord>> {
        if !self.is_indexed(ctx) {
            return self.rpc.account_ats(ctx, address, req, filters).await;
        }
        let ss58_prefix = self.ss58_prefix(ctx);
        let sid = self.network_sid(ctx)?;
        queries::ats::list_account_ats_page(&self.pool, sid, address, &req, &filters, ss58_prefix)
            .await
    }

    // Token allocation: not indexed yet — defer to the RPC fallback so
    // its `Unimplemented` (or future real impl) flows through unchanged.
    async fn token_overview(&self, ctx: ChainCtx) -> DataResult<TokenOverview> {
        self.rpc.token_overview(ctx).await
    }

    async fn envelope_detail(
        &self,
        ctx: ChainCtx,
        id: EnvelopeId,
    ) -> DataResult<Option<EnvelopeDetail>> {
        self.rpc.envelope_detail(ctx, id).await
    }

    async fn account_allocations(
        &self,
        ctx: ChainCtx,
        address: &str,
    ) -> DataResult<Vec<Allocation>> {
        self.rpc.account_allocations(ctx, address).await
    }

    async fn runtime_details(&self, ctx: ChainCtx, at: Option<u64>) -> DataResult<RuntimeDetails> {
        // No DB path for the point-in-time snapshot — the `:code` blob
        // and the full `Core_version` decode both live on the chain,
        // and the `runtime_versions` table in the schema is reserved
        // for Phase 2 (metadata blob persistence). Pass through to the
        // RPC provider, which already caches the head-path read.
        self.rpc.runtime_details(ctx, at).await
    }

    async fn runtime_upgrades(&self, ctx: ChainCtx) -> DataResult<Vec<RuntimeUpgrade>> {
        // Non-indexed networks haven't got a `blocks` history for us to
        // aggregate, so fall through to the RPC path (which returns a
        // single "current only" entry). Indexed networks run the
        // `SELECT DISTINCT ON (spec_version)` aggregate and reconcile
        // `is_current` against the live `runtime_identity()` value.
        if !self.is_indexed(ctx) {
            return self.rpc.runtime_upgrades(ctx).await;
        }
        let sid = self.network_sid(ctx)?;
        let mut upgrades = queries::runtime::runtime_upgrades(&self.pool, sid).await?;

        // Figure out the active spec version so the history row that
        // matches it gets flagged. We read through the RPC client —
        // cheaper than decoding metadata from the indexer state, and
        // the value is cached in-process.
        let current_spec = self
            .rpc
            .runtime_details(ctx, None)
            .await
            .ok()
            .map(|d| d.identity.spec_version);
        if let Some(current) = current_spec {
            // Multiple rows could in principle share the same
            // `spec_version` if the indexer replayed a chain reorg,
            // but the aggregate collapses duplicates by construction;
            // the check here is still cheap and forward-compatible.
            for u in upgrades.iter_mut() {
                if u.spec_version == current {
                    u.is_current = true;
                }
            }
            // If the DB hasn't caught up to the current spec yet
            // (fresh deploy, or the live runtime just upgraded but
            // we haven't indexed a post-upgrade block yet), prepend a
            // synthetic "current" row so the UI still shows the live
            // spec. `first_block = None` signals "start unknown" so
            // the frontend can render this row without a block link —
            // distinct from `Some(0)` which would mean "deployed at
            // genesis", the legit value for a chain on its first spec.
            if !upgrades.iter().any(|u| u.spec_version == current) {
                upgrades.insert(
                    0,
                    RuntimeUpgrade {
                        spec_version: current,
                        first_block: None,
                        first_block_timestamp_ms: None,
                        is_current: true,
                    },
                );
            }
        }
        Ok(upgrades)
    }

    async fn subscribe_blocks(&self, ctx: ChainCtx) -> DataResult<BoxStream<Block>> {
        self.rpc.subscribe_blocks(ctx).await
    }

    async fn subscribe_transfers(&self, ctx: ChainCtx) -> DataResult<BoxStream<Transfer>> {
        if !self.is_indexed(ctx) {
            return self.rpc.subscribe_transfers(ctx).await;
        }
        match self.buffer_for(ctx) {
            Some(buffer) => {
                let buf = buffer.read().await;
                Ok(box_broadcast(buf.subscribe_transfers()))
            }
            None => self.rpc.subscribe_transfers(ctx).await,
        }
    }

    async fn subscribe_ats_feed(&self, ctx: ChainCtx) -> DataResult<BoxStream<AtsFeedItem>> {
        self.rpc.subscribe_ats_feed(ctx).await
    }
}

/// Shared cursor parser for the block list path. Handlers and the
/// provider both need a precise `400` on a malformed cursor, never a
/// silent fallback to "no cursor" — a cursor that no longer round-trips
/// must surface loudly instead of quietly recycling the first page.
fn parse_block_cursor(req: &PageRequest) -> DataResult<Option<BlockCursor>> {
    let Some(raw) = req.cursor.as_deref() else {
        return Ok(None);
    };
    raw.parse::<BlockCursor>()
        .map(Some)
        .map_err(|e| DataError::BadRequest(e.to_string()))
}

fn parse_extrinsic_cursor(req: &PageRequest) -> DataResult<Option<ExtrinsicCursor>> {
    let Some(raw) = req.cursor.as_deref() else {
        return Ok(None);
    };
    raw.parse::<ExtrinsicCursor>()
        .map(Some)
        .map_err(|e| DataError::BadRequest(e.to_string()))
}

/// Wrap a `tokio::broadcast::Receiver` into the trait-object stream
/// shape. Lag notifications are dropped with a trace: surfacing them to
/// the UI layer isn't actionable, and the subscriber stays alive for
/// the next item.
fn box_broadcast<T: Clone + Send + 'static>(rx: broadcast::Receiver<T>) -> BoxStream<T> {
    Box::pin(BroadcastStream::new(rx).filter_map(|r| async move {
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::debug!(error = %e, "buffer broadcast lag: dropping items");
                None
            }
        }
    }))
}
