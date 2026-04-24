//! `PendingBuffer` — the in-RAM ring buffer covering the
//! `[finalized+1 … best]` window.
//!
//! Phase 3 scope (see `docs/indexing-plan.md`): a minimal,
//! single-process container that the `IndexedProvider` can consult for
//! tip-level extrinsic lookups. The full subscription fan-out +
//! WebSocket integration + reorg handling lands with later phases;
//! this module ships just the data structure + a thread-safe wrapper
//! so:
//!
//! * the routing in [`crate::data::indexed::provider`] can fall
//!   through DB → buffer consistently from the start,
//! * the Phase 3 unit/integration tests can inject synthetic best
//!   blocks and exercise the "tip lookup returns a non-finalized
//!   extrinsic" branch without standing up a subxt subscription.
//!
//! Nothing in this file talks to Postgres or subxt. A later phase will
//! wire a `subscribe_best` producer that calls [`PendingBuffer::append_block`]
//! on every new head and [`PendingBuffer::advance_finalized`] on every
//! finalized-head tick, but the data shape won't change.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use tokio::sync::{broadcast, RwLock};

use crate::data::ss58::decode_hex32;
use crate::domain::{Extrinsic, Transfer};

/// Per-channel broadcast capacity. Sized to absorb a handful of
/// finalized blocks worth of items if a slow subscriber falls behind —
/// dropping items is acceptable (the receiver re-hydrates from the
/// `latest_*` queries on reconnect) but a too-small ring would force
/// frequent lag drops at the tip.
const BUFFER_BROADCAST_CAPACITY: usize = 256;

/// Hard cap on how many best-blocks we hold concurrently. Melodie
/// finality is a handful of blocks behind the tip under normal
/// conditions; 64 leaves a comfortable margin for a slow finalizer
/// without letting the buffer grow unbounded if finalization stalls.
/// If the chain wedges past this point we drop the oldest entries —
/// they're no longer the "best" chain anyway, the DB will catch up
/// once finalization resumes.
pub const BUFFER_CAPACITY: usize = 64;

/// Per-block entry. Mirrors the `IndexedBlock` sketch in the plan's §3
/// but trimmed to the fields Phase 3 actually needs: block number +
/// hash (for lookups) and the decoded extrinsic list (for the tip
/// lookup test case). Transfers / balance_movements / events will
/// plug in with the corresponding phases.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferedBlock {
    pub number: u64,
    pub hash: [u8; 32],
    pub extrinsics: Vec<Extrinsic>,
    /// `Balances.Transfer` events projected from the block, in
    /// chain-emission order. Phase 4 wires this so the buffer can
    /// answer `subscribe_transfers` + the tip slice of
    /// `latest_transfers` without a DB round-trip; an empty vec is
    /// the right value for a block with no transfers (don't drop the
    /// block from the buffer just because nothing was sent).
    pub transfers: Vec<Transfer>,
}

/// Pure in-memory container. Wrap in `Arc<RwLock<_>>` for concurrent
/// access — callers read far more often than they write, so an
/// `RwLock` beats a `Mutex` even at the tip cadence.
///
/// Indexes:
///
/// * `by_number` — number → position in the `VecDeque`. O(1) block
///   lookup and constant-time reorg truncation.
/// * `by_xt_hash` — extrinsic hash → `(block_num, idx)`. Duplicate
///   hashes (same extrinsic observed on competing forks) overwrite:
///   the latest append wins, which is what the UI wants when
///   resolving `/extrinsic/<hash>` against the tip.
pub struct PendingBuffer {
    /// Network this buffer tracks. Every metrics emit labels with it so
    /// dashboards can slice per-chain.
    network_id: &'static str,
    blocks: VecDeque<BufferedBlock>,
    by_number: HashMap<u64, usize>,
    by_xt_hash: HashMap<[u8; 32], (u64, u32)>,
    finalized_head: u64,
    /// Fan-out for transfers projected by every newly appended block.
    /// Held by the buffer rather than the surrounding worker so a
    /// caller that owns only the buffer (tests, the IndexedProvider)
    /// can subscribe without reaching across module boundaries. The
    /// sender stays alive for the buffer's lifetime so re-subscribing
    /// after a temporary lag is safe.
    transfers_tx: broadcast::Sender<Transfer>,
}

impl PendingBuffer {
    pub fn new(network_id: &'static str) -> Self {
        Self::with_capacity(network_id, BUFFER_CAPACITY)
    }

    pub fn with_capacity(network_id: &'static str, cap: usize) -> Self {
        let (transfers_tx, _) = broadcast::channel(BUFFER_BROADCAST_CAPACITY);
        Self {
            network_id,
            blocks: VecDeque::with_capacity(cap),
            by_number: HashMap::new(),
            by_xt_hash: HashMap::new(),
            finalized_head: 0,
            transfers_tx,
        }
    }

    pub fn network_id(&self) -> &'static str {
        self.network_id
    }

    /// Current best-head block number, if any.
    pub fn best_head(&self) -> Option<u64> {
        self.blocks.back().map(|b| b.number)
    }

    /// Latest finalized block number observed. Entries at or below
    /// this number are evicted — their canonical view now lives in
    /// Postgres.
    pub fn finalized_head(&self) -> u64 {
        self.finalized_head
    }

    /// Append one best block. Evicts the oldest entry if the buffer
    /// has hit its cap — no attempt at reorg detection yet, that's
    /// deferred to the later phase that also wires the subxt
    /// subscription.
    ///
    /// Side effect: every transfer carried on the block is fanned out
    /// to [`subscribe_transfers`] receivers in chain-emission order. A
    /// `send` failure (no live subscribers) is silently swallowed —
    /// that's the documented behaviour of `tokio::broadcast` and the
    /// buffer doesn't care whether anyone is listening.
    pub fn append_block(&mut self, block: BufferedBlock) {
        if self.blocks.len() == BUFFER_CAPACITY {
            if let Some(evicted) = self.blocks.pop_front() {
                self.by_number.remove(&evicted.number);
                self.reindex_positions();
                self.rebuild_xt_index();
            }
        }
        let pos = self.blocks.len();
        for ext in &block.extrinsics {
            if let Some(hash) = decode_hex32(&ext.hash) {
                self.by_xt_hash.insert(hash, (block.number, ext.index));
            }
        }
        for transfer in &block.transfers {
            // Ignore the SendError: no subscribers means nothing to
            // do, the buffer state itself isn't affected.
            let _ = self.transfers_tx.send(transfer.clone());
        }
        self.by_number.insert(block.number, pos);
        self.blocks.push_back(block);
        self.publish_metrics();
    }

    /// Drop entries at or below `up_to`. Called whenever a finalized
    /// head tick arrives from the RPC watch — those blocks now live
    /// in Postgres and the buffer would double-count on reads.
    pub fn advance_finalized(&mut self, up_to: u64) {
        self.finalized_head = self.finalized_head.max(up_to);
        while let Some(front) = self.blocks.front() {
            if front.number <= up_to {
                let evicted = self.blocks.pop_front().unwrap();
                self.by_number.remove(&evicted.number);
            } else {
                break;
            }
        }
        self.reindex_positions();
        self.rebuild_xt_index();
        self.publish_metrics();
    }

    /// Lookup an extrinsic by its 32-byte hash. Returns the extrinsic
    /// paired with the block number it came from — callers use the
    /// block to render "pending" state when the number is above
    /// [`Self::finalized_head`].
    pub fn extrinsic_by_hash(&self, hash: &[u8; 32]) -> Option<Extrinsic> {
        let (block_num, idx) = self.by_xt_hash.get(hash).copied()?;
        self.extrinsic_by_block_idx(block_num, idx)
    }

    /// Lookup by canonical (block_num, idx). Returns a cloned
    /// [`Extrinsic`] — the domain type derives `Clone` and the buffer
    /// is small enough that the clone cost is irrelevant compared to
    /// the DB round-trip we'd otherwise pay.
    pub fn extrinsic_by_block_idx(&self, block_num: u64, idx: u32) -> Option<Extrinsic> {
        let pos = *self.by_number.get(&block_num)?;
        let block = self.blocks.get(pos)?;
        block.extrinsics.iter().find(|e| e.index == idx).cloned()
    }

    /// All buffered blocks, newest first. Small helper used by the
    /// `latest_extrinsics` merge path: the provider concatenates
    /// buffer-first + DB-older and dedups by `(block, idx)` so a block
    /// that's finalized mid-query never appears twice.
    pub fn iter_blocks_newest_first(&self) -> impl DoubleEndedIterator<Item = &BufferedBlock> {
        self.blocks.iter().rev()
    }

    /// New `Receiver` for the transfer fan-out. Items land as
    /// [`append_block`] is called for blocks carrying transfers;
    /// historical replay isn't part of the contract — the
    /// `latest_transfers` query handles cold start.
    pub fn subscribe_transfers(&self) -> broadcast::Receiver<Transfer> {
        self.transfers_tx.subscribe()
    }

    /// Number of buffered blocks. Useful for metrics + tests.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// After `pop_front`, every remaining block's slot moved left by
    /// one. Small loop (≤ [`BUFFER_CAPACITY`] entries) so we don't
    /// bother with a more clever index.
    fn reindex_positions(&mut self) {
        self.by_number.clear();
        for (i, b) in self.blocks.iter().enumerate() {
            self.by_number.insert(b.number, i);
        }
    }

    /// Rebuild the xt-hash → (block_num, idx) map from the surviving
    /// blocks. Called after eviction: a dropped block's hashes must
    /// not keep resolving to stale coordinates, and rebuilding is
    /// O(blocks × extrinsics per block) which is bounded by
    /// `BUFFER_CAPACITY × realistic_ext_count`.
    fn rebuild_xt_index(&mut self) {
        self.by_xt_hash.clear();
        for b in &self.blocks {
            for ext in &b.extrinsics {
                if let Some(h) = decode_hex32(&ext.hash) {
                    self.by_xt_hash.insert(h, (b.number, ext.index));
                }
            }
        }
    }

    /// Push `buffer_size` / `buffer_best_head` / `buffer_finalized_head`
    /// into the global Prometheus registry, labelled by this buffer's
    /// `network_id`. Called after every mutation so a scrape that
    /// lands moments later sees the real tip.
    fn publish_metrics(&self) {
        crate::server::metrics::record_buffer_state(
            self.network_id,
            self.blocks.len(),
            self.best_head(),
            self.finalized_head,
        );
    }
}

/// Thread-safe shared handle. `Arc<RwLock<PendingBuffer>>` is the
/// shape passed into [`crate::data::indexed::IndexedProvider`] (and
/// eventually the WS fan-out). Kept behind a type alias so we can
/// swap the wrapper later (sharded RwLock, DashMap, …) without
/// touching every caller.
pub type SharedBuffer = Arc<RwLock<PendingBuffer>>;

/// Convenience constructor so call sites don't spell out the wrapper
/// type on the hot boot path. One buffer per indexed network.
pub fn shared(network_id: &'static str) -> SharedBuffer {
    Arc::new(RwLock::new(PendingBuffer::new(network_id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{CallResult, ExtrinsicArgs};

    /// Build a throwaway `Extrinsic` with the hash pre-rendered as
    /// the `0x…` string the domain uses on the wire. Kept inline so
    /// the tests stay self-contained.
    fn make_extrinsic(block: u64, idx: u32, hash_byte: u8) -> Extrinsic {
        let hash_bytes = [hash_byte; 32];
        Extrinsic {
            id: format!("{block}-{idx}"),
            block_number: block,
            index: idx,
            hash: format!(
                "0x{}",
                hash_bytes
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            ),
            module: "Balances".into(),
            call: "transfer_keep_alive".into(),
            signed: true,
            signer: Some("5GrwvaEFinqsLPxV6dRiYpZkmf3uG9bAagdNXTuZmrjN6dhV".into()),
            args: ExtrinsicArgs::Raw { hex: "0x".into() },
            result: CallResult::Success,
            nonce: Some(0),
            tip: 0,
            fee: 100,
            timestamp_ms: 0,
            events: Vec::new(),
        }
    }

    fn make_block(number: u64, hash_byte: u8, ext_hash_bytes: &[u8]) -> BufferedBlock {
        BufferedBlock {
            number,
            hash: [hash_byte; 32],
            extrinsics: ext_hash_bytes
                .iter()
                .enumerate()
                .map(|(i, &b)| make_extrinsic(number, i as u32, b))
                .collect(),
            transfers: Vec::new(),
        }
    }

    /// Happy path: append one best block, look the extrinsic up by
    /// its raw 32-byte hash, assert the returned domain value round-trips.
    /// Locks the `0x…` ↔ bytes contract — a regression in the hex
    /// decoder would silently drop lookups.
    #[test]
    fn extrinsic_lookup_by_hash_round_trips() {
        let mut buf = PendingBuffer::new("test");
        buf.append_block(make_block(100, 0xaa, &[0x11, 0x22]));

        let hash_11 = [0x11u8; 32];
        let got = buf.extrinsic_by_hash(&hash_11).expect("hash known");
        assert_eq!(got.block_number, 100);
        assert_eq!(got.index, 0);

        let hash_22 = [0x22u8; 32];
        let got = buf.extrinsic_by_hash(&hash_22).expect("second hash known");
        assert_eq!(got.index, 1);
    }

    /// Lookup by `(block, idx)` hits the `by_number` index and avoids
    /// a linear scan of the deque. Both indexes must stay consistent
    /// with the block contents — this test locks that invariant.
    #[test]
    fn extrinsic_lookup_by_block_idx_matches_by_hash() {
        let mut buf = PendingBuffer::new("test");
        buf.append_block(make_block(42, 0xff, &[0x33]));

        let via_idx = buf.extrinsic_by_block_idx(42, 0).expect("indexed");
        let via_hash = buf.extrinsic_by_hash(&[0x33u8; 32]).expect("hashed");
        assert_eq!(via_idx, via_hash);
    }

    /// `advance_finalized(N)` drops every entry at or below `N`. The
    /// xt-hash index must be rebuilt afterwards or we'd resolve a
    /// finalized extrinsic from the buffer instead of letting the DB
    /// answer — which is the UX bug this test locks.
    #[test]
    fn advance_finalized_evicts_and_clears_indexes() {
        let mut buf = PendingBuffer::new("test");
        buf.append_block(make_block(10, 0x01, &[0xaa]));
        buf.append_block(make_block(11, 0x02, &[0xbb]));
        buf.append_block(make_block(12, 0x03, &[0xcc]));

        buf.advance_finalized(11);
        assert_eq!(buf.len(), 1, "only block 12 should remain");
        assert_eq!(buf.best_head(), Some(12));
        assert!(buf.extrinsic_by_hash(&[0xaau8; 32]).is_none());
        assert!(buf.extrinsic_by_hash(&[0xbbu8; 32]).is_none());
        assert!(buf.extrinsic_by_hash(&[0xccu8; 32]).is_some());
    }

    /// Phase 4 contract: appending a block fans every transfer out
    /// to live `subscribe_transfers` receivers in chain-emission
    /// order. Subscribers attached *before* the append must see the
    /// items; one attached after won't (broadcast is fire-and-forget,
    /// no replay) — this test only guards the "before" path because
    /// the IndexedProvider always subscribes during HTTP boot, well
    /// ahead of any best-block append.
    #[tokio::test]
    async fn append_block_fans_transfers_out_to_subscribers() {
        let mut buf = PendingBuffer::new("test");
        let mut rx = buf.subscribe_transfers();

        // Two transfers on a single block — order matters because the
        // UI renders them as a chronological stream and a reordering
        // here would silently shuffle the live feed.
        let block = BufferedBlock {
            number: 50,
            hash: [0x10; 32],
            extrinsics: Vec::new(),
            transfers: vec![
                make_transfer("alice", "bob", 100),
                make_transfer("carol", "dave", 250),
            ],
        };
        buf.append_block(block);

        let first = rx.recv().await.expect("first transfer");
        let second = rx.recv().await.expect("second transfer");
        assert_eq!(first.from, "alice");
        assert_eq!(first.amount, 100);
        assert_eq!(second.from, "carol");
        assert_eq!(second.amount, 250);
    }

    /// A block carrying no transfers must *not* drop the subscriber
    /// or send anything spurious — `latest_transfers` over a quiet
    /// window would otherwise look populated by phantom items.
    #[tokio::test]
    async fn append_block_with_no_transfers_emits_nothing() {
        let mut buf = PendingBuffer::new("test");
        let mut rx = buf.subscribe_transfers();
        buf.append_block(BufferedBlock {
            number: 1,
            hash: [0x01; 32],
            extrinsics: Vec::new(),
            transfers: Vec::new(),
        });
        // `try_recv` returns `Empty` immediately when the channel has
        // no items — zero-value blocks must not enqueue anything.
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }

    fn make_transfer(from: &str, to: &str, amount: u128) -> Transfer {
        Transfer {
            extrinsic: make_extrinsic(0, 0, 0),
            from: from.into(),
            to: to.into(),
            amount,
        }
    }

    /// Nothing to find ⇒ `None`, not a panic. Integration paths hit
    /// this on every lookup before the first block lands.
    #[test]
    fn empty_buffer_returns_none() {
        let buf = PendingBuffer::new("test");
        assert!(buf.is_empty());
        assert!(buf.best_head().is_none());
        assert!(buf.extrinsic_by_hash(&[0u8; 32]).is_none());
        assert!(buf.extrinsic_by_block_idx(0, 0).is_none());
    }
}
