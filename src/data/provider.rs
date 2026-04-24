//! The [`ChainData`] trait — the explorer's single source of chain data.
//!
//! Every page (via server functions, eventually) reads through this trait, so
//! swapping the mock generator for the subxt RPC client is a one-line change
//! in [`crate::server::state`]. The method surface mirrors the current mock
//! API so migration is incremental: start routing one query at a time, keep
//! the rest on the mock until each RPC path is ready.

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use crate::domain::{
    Account, Allocation, AtsFeedItem, AtsRecord, AtsStats, Block, BlockEvent, EnvelopeDetail,
    EnvelopeId, Extrinsic, Page, PageRequest, RuntimeDetails, RuntimeUpgrade, TokenOverview,
    Transfer,
};
use crate::network::ChainCtx;

use super::error::{DataError, DataResult};
use super::filters::{
    AccountAtsFilters, AtsFeedFilters, AtsFilters, BlockFilters, EventFilters, ExtrinsicFilters,
    TransferFilters,
};

/// Network id of the chain that ships `pallet-token-allocation`. Every token
/// endpoint gates on this before touching storage so off-chain networks get a
/// clean `NotSupported` (→ 404) instead of a runtime-metadata mismatch.
pub(crate) const TOKEN_PALLET_NETWORK: &str = "allfeat";

/// Short-circuit the three token endpoints when the ctx points at a network
/// without `pallet-token-allocation`.
pub(crate) fn ensure_token_network(ctx: ChainCtx, what: &'static str) -> DataResult<()> {
    if ctx.spec.id == TOKEN_PALLET_NETWORK {
        Ok(())
    } else {
        Err(DataError::NotSupported {
            network: ctx.spec.id,
            what,
        })
    }
}

/// Object-safe stream handle returned by every `subscribe_*` method. Keeping
/// it behind a trait alias means implementations can pick their own producer
/// shape (tokio broadcast, a mock interval task, a future subxt stream
/// mapping) without leaking concrete types through the trait surface.
pub type BoxStream<T> = Pin<Box<dyn Stream<Item = T> + Send>>;

#[async_trait]
pub trait ChainData: Send + Sync + 'static {
    // ── Readiness ───────────────────────────────────────────────────────────
    /// Cheap synchronous readiness probe for `/readyz`. Default returns
    /// `true` because mock-style providers are always ready the moment the
    /// struct exists. RPC-backed providers override it to confirm at least
    /// one upstream node has produced a finalized head — until the head
    /// subscription fires we'd happily serve stale "head" numbers derived
    /// from fallback paths.
    fn is_ready(&self) -> bool {
        true
    }

    // ── Chain tip ───────────────────────────────────────────────────────────
    /// Current head block number (finalized tip on the RPC path, derived
    /// from the mock clock for [`super::mock::MockProvider`]). Pages must
    /// go through this accessor rather than computing `ctx.head_block()`
    /// themselves, otherwise the mock-derived head leaks into RPC mode and
    /// every head-relative window (pagination, "latest blocks", dashboard
    /// hero) requests block numbers past the real chain tip.
    async fn head_block(&self, ctx: ChainCtx) -> DataResult<u64>;

    // ── Blocks ──────────────────────────────────────────────────────────────
    async fn block_by_number(&self, ctx: ChainCtx, number: u64) -> DataResult<Option<Block>>;

    /// Paginated newest-first block list. The window starts at the tip when
    /// `req.cursor` is `None`, otherwise at the block strictly below the
    /// cursor's block number. `req.count` caps the number of rows returned;
    /// the implementation uses a fetch-N+1 trick internally to set
    /// [`crate::domain::PageInfo::has_more`] without an extra `COUNT(*)`.
    /// [`crate::domain::PageInfo::total`] is populated from the chain head
    /// (cheap) so the UI can render a total without a separate call.
    async fn latest_blocks(
        &self,
        ctx: ChainCtx,
        req: PageRequest,
        filters: BlockFilters,
    ) -> DataResult<Page<Block>>;

    // ── Extrinsics ──────────────────────────────────────────────────────────
    async fn extrinsics_in_block(&self, ctx: ChainCtx, block: u64) -> DataResult<Vec<Extrinsic>>;

    /// Every event emitted in `block`, including the Initialization and
    /// Finalization phases that [`extrinsics_in_block`] filters out. Used
    /// by the block detail page's Events tab so session-change effects
    /// (`session.NewSession`, `grandpa.NewAuthorities`, …) are visible
    /// instead of silently dropped.
    async fn events_in_block(&self, ctx: ChainCtx, block: u64) -> DataResult<Vec<BlockEvent>>;

    /// Paginated newest-first chain-wide event feed. Cursor is
    /// `<block>-<phase>-<idx>` (see `src/data/cursor.rs`); the phase
    /// letter is purely decorative for the URL — ordering is still
    /// `(block DESC, event_idx DESC)`. `total` stays `None` for the
    /// same reason as extrinsics: row counts at full-chain scale need
    /// an `indexer_counters` table the plan defers. Unlike
    /// [`latest_extrinsics`], this preserves `Initialization` and
    /// `Finalization` events so the dashboard feed shows session-change
    /// effects alongside ordinary extrinsic events.
    async fn latest_events(
        &self,
        ctx: ChainCtx,
        req: PageRequest,
        filters: EventFilters,
    ) -> DataResult<Page<BlockEvent>>;

    /// Paginated newest-first extrinsic list. The timestamp inherent is
    /// stripped at the storage layer so the window stays focused on
    /// user-visible calls. `total` is not populated — counts at this
    /// scale would need an `indexer_counters` table the plan defers.
    async fn latest_extrinsics(
        &self,
        ctx: ChainCtx,
        req: PageRequest,
        filters: ExtrinsicFilters,
    ) -> DataResult<Page<Extrinsic>>;

    async fn extrinsic_by_id(&self, ctx: ChainCtx, id: &str) -> DataResult<Option<Extrinsic>>;

    // ── Transfers ───────────────────────────────────────────────────────────
    /// Paginated newest-first `Balances.Transfer` feed. Cursor shape is
    /// `<block>-<event_idx>` so batched transfers (multiple events in one
    /// extrinsic) paginate cleanly. `total` is not populated.
    async fn latest_transfers(
        &self,
        ctx: ChainCtx,
        req: PageRequest,
        filters: TransferFilters,
    ) -> DataResult<Page<Transfer>>;

    // ── Accounts ────────────────────────────────────────────────────────────
    async fn account_by_address(&self, ctx: ChainCtx, address: &str)
        -> DataResult<Option<Account>>;

    async fn top_accounts(&self, ctx: ChainCtx, count: u32) -> DataResult<Vec<Account>>;

    // ── ATS ─────────────────────────────────────────────────────────────────
    async fn ats_stats(&self, ctx: ChainCtx) -> DataResult<AtsStats>;

    async fn ats_by_index(&self, ctx: ChainCtx, index: u32) -> DataResult<Option<AtsRecord>>;

    /// Paginated newest-first ATS list. Cursor is
    /// [`crate::data::cursor::AtsCursor`] (the ATS id); `total` is
    /// populated from the registry count (cheap single-row aggregate).
    async fn ats_list(
        &self,
        ctx: ChainCtx,
        req: PageRequest,
        filters: AtsFilters,
    ) -> DataResult<Page<AtsRecord>>;

    /// Paginated newest-first per-ATS-version feed. Cursor is
    /// [`crate::data::cursor::AtsFeedCursor`]; ordering is
    /// `(ats_id DESC, version DESC)`.
    async fn ats_version_feed(
        &self,
        ctx: ChainCtx,
        req: PageRequest,
        filters: AtsFeedFilters,
    ) -> DataResult<Page<AtsFeedItem>>;

    /// Paginated ATS list scoped to a single owner. Cursor shape matches
    /// [`ats_list`] so the frontend can reuse one composable.
    async fn account_ats(
        &self,
        ctx: ChainCtx,
        address: &str,
        req: PageRequest,
        filters: AccountAtsFilters,
    ) -> DataResult<Page<AtsRecord>>;

    // ── Token allocation (mainnet-only pallet) ──────────────────────────────
    //
    // Implementations on networks without `pallet-token-allocation` must
    // return [`super::error::DataError::NotSupported`] so the API surface
    // lands as a clean 404 rather than a 500.

    async fn token_overview(&self, ctx: ChainCtx) -> DataResult<TokenOverview>;

    async fn envelope_detail(
        &self,
        ctx: ChainCtx,
        id: EnvelopeId,
    ) -> DataResult<Option<EnvelopeDetail>>;

    async fn account_allocations(
        &self,
        ctx: ChainCtx,
        address: &str,
    ) -> DataResult<Vec<Allocation>>;

    // ── Runtime identity & upgrades ─────────────────────────────────────────
    //
    // Both endpoints back `/api/v1/networks/{id}/runtime*` and sit outside the
    // paginated-list shape. `runtime_details` is a point-in-time snapshot
    // (defaults to the finalized head when `at` is `None`); `runtime_upgrades`
    // walks history — DB-backed on the indexed path, a single-row "current"
    // entry on the RPC / mock fallbacks.

    /// Full runtime identity, `:code` fingerprint, genesis hash, metadata
    /// version — the payload the runtime page renders in one fetch. When
    /// `at` is `Some(N)` the snapshot is taken against block `N`; `None`
    /// means "at the finalized head".
    async fn runtime_details(&self, ctx: ChainCtx, at: Option<u64>) -> DataResult<RuntimeDetails>;

    /// Historical `system.set_code` timeline, newest-first. One row per
    /// distinct `spec_version` seen in the indexed history (or a
    /// single-row "current only" entry on the RPC fallback, which has no
    /// per-block history).
    async fn runtime_upgrades(&self, ctx: ChainCtx) -> DataResult<Vec<RuntimeUpgrade>>;

    // ── Live streams ────────────────────────────────────────────────────────
    //
    // These feed the live WebSocket endpoint (`crate::live::server`). Each
    // implementation owns the producer side: `MockProvider` spawns a
    // synthetic tick task, `RpcProvider` rides the existing finalized-head
    // subscription. Subscribers only observe items produced *after* the
    // subscribe call — initial page state is still served by the `latest_*`
    // methods above so SSR HTML stays identical to today.
    //
    // Streams are infallible at the item level by design: if the producer
    // dies the stream simply ends, and the client re-hydrates by
    // reconnecting. Surfacing per-item errors through the stream would leak
    // backend state into the wire protocol without helping the UI — the
    // dashboard can't do anything more useful than "drop the row" anyway.

    /// Stream of newly seen blocks (finalized on RPC, synthesized on mock).
    /// The first item lands whenever the producer emits next — never a
    /// replay of historical blocks.
    async fn subscribe_blocks(&self, ctx: ChainCtx) -> DataResult<BoxStream<Block>>;

    /// Stream of transfers extracted from each new block, one item per
    /// transfer in block order. A block with no transfers yields nothing.
    async fn subscribe_transfers(&self, ctx: ChainCtx) -> DataResult<BoxStream<Transfer>>;

    /// Stream of ATS feed items emitted when a new version is registered.
    /// A block with no ATS activity yields nothing.
    async fn subscribe_ats_feed(&self, ctx: ChainCtx) -> DataResult<BoxStream<AtsFeedItem>>;
}
