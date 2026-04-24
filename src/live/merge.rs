//! Dedup + merge helpers shared by pages.
//!
//! Compiled on both sides: the SSR render path sees an empty live buffer
//! and [`merge_live`] degenerates to "copy the initial list", so the
//! server-rendered HTML matches what a freshly-hydrated client with no
//! stream updates would produce. Pages can therefore wrap the Resource
//! result in `merge_live(...)` unconditionally.

use std::collections::VecDeque;

use crate::domain::{AtsFeedItem, Block, Transfer};

/// Stable identity of a streamed item — the field(s) that make "same
/// thing" unambiguous. Lives on the live-stream side rather than on the
/// domain types because it's strictly a client-side UI concern (the
/// server doesn't care about collisions, it just pushes every item).
pub trait LiveItem {
    type Key: Eq + std::hash::Hash + Clone;
    fn dedup_key(&self) -> Self::Key;
}

impl LiveItem for Block {
    type Key = u64;
    fn dedup_key(&self) -> u64 {
        self.number
    }
}

impl LiveItem for Transfer {
    type Key = String;
    fn dedup_key(&self) -> String {
        self.extrinsic.id.clone()
    }
}

impl LiveItem for AtsFeedItem {
    type Key = (u32, u32);
    fn dedup_key(&self) -> (u32, u32) {
        (self.ats_id, self.version_index)
    }
}

/// Interleave the live buffer (newest-first) with the SSR-seeded Vec,
/// drop duplicates by [`LiveItem::dedup_key`], and cap at `limit`. Called
/// from the view render closure: the page keeps Resource for the initial
/// render and plumbs the live signal in as a second source of truth.
///
/// Ordering invariants:
///
/// * `live` items come first (they're strictly newer than Resource items
///   received after hydration — the server emits in chronological order).
/// * `initial` fills the rest up to `limit`.
/// * Ties on `dedup_key` are resolved in favour of whichever comes first
///   in the merged stream, which for blocks/transfers means the live
///   copy wins. The initial Resource copy would be identical anyway.
pub fn merge_live<T>(live: VecDeque<T>, initial: Vec<T>, limit: usize) -> Vec<T>
where
    T: LiveItem + Clone,
{
    // Explorer lists cap at 25 items, so a linear scan beats a HashSet: we
    // skip the per-render allocation + per-item hash while still early-exiting
    // at `limit`. If a caller bumps `limit` above ~50, switching back to a
    // HashSet would win again.
    let mut keys: Vec<T::Key> = Vec::with_capacity(limit);
    let mut out: Vec<T> = Vec::with_capacity(limit);
    for item in live.into_iter().chain(initial) {
        let key = item.dedup_key();
        if keys.iter().any(|k| k == &key) {
            continue;
        }
        keys.push(key);
        out.push(item);
        if out.len() >= limit {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(n: u64) -> Block {
        Block {
            number: n,
            hash: format!("0x{n:064x}"),
            parent_hash: String::new(),
            state_root: String::new(),
            extrinsics_root: String::new(),
            timestamp_ms: n as i64 * 1000,
            finalized: true,
            extrinsic_count: 0,
            event_count: 0,
            author: String::new(),
            author_name: String::new(),
            ref_time: 0,
            ref_time_pct: 0,
            proof_size: 0,
            spec_version: 0,
            size_bytes: 0,
        }
    }

    #[test]
    fn empty_live_copies_initial_up_to_limit() {
        let initial: Vec<_> = (1..=10).map(block).collect();
        let merged = merge_live::<Block>(VecDeque::new(), initial, 5);
        assert_eq!(merged.len(), 5);
        assert_eq!(merged[0].number, 1);
        assert_eq!(merged[4].number, 5);
    }

    #[test]
    fn live_items_come_first_and_initial_fills_to_limit() {
        let mut live = VecDeque::new();
        live.push_back(block(100));
        live.push_back(block(99));
        let initial = vec![block(98), block(97), block(96)];
        let merged = merge_live(live, initial, 4);
        assert_eq!(
            merged.iter().map(|b| b.number).collect::<Vec<_>>(),
            vec![100, 99, 98, 97]
        );
    }

    #[test]
    fn duplicate_blocks_emit_once() {
        let mut live = VecDeque::new();
        live.push_back(block(5));
        // Same block number appears in the seeded history too.
        let initial = vec![block(5), block(4), block(3)];
        let merged = merge_live(live, initial, 10);
        assert_eq!(
            merged.iter().map(|b| b.number).collect::<Vec<_>>(),
            vec![5, 4, 3]
        );
    }

    #[test]
    fn limit_caps_output_even_when_live_alone_overflows() {
        let live: VecDeque<_> = (0..20u64).map(|i| block(100 + i)).collect();
        let merged = merge_live::<Block>(live, Vec::new(), 5);
        assert_eq!(merged.len(), 5);
    }

    #[test]
    fn limit_one_stops_after_first_push() {
        let mut live = VecDeque::new();
        live.push_back(block(100));
        let initial = vec![block(99), block(98)];
        let merged = merge_live(live, initial, 1);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].number, 100);
    }
}
