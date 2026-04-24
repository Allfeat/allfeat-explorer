//! Collect the unique set of accounts touched by a block so the indexer
//! can fetch their post-block `System::Account` snapshot and UPSERT it
//! into `account_balances`.
//!
//! Why a collector and not an aggregator: the previous pipeline derived
//! per-account signed deltas from events and added them to a running
//! total. That approach accumulates drift every time a pallet outside
//! `pallet_balances` mutates free/reserved (staking, treasury, identity,
//! vesting, the modern `fungible::hold` API, custom Allfeat pallets…)
//! because the projection has to enumerate every variant that can move
//! tokens, forever. The snapshot pipeline is agnostic: whatever pallet
//! mutated the account, `System::Account` at the block's hash is the
//! canonical answer — we just need to know *which* accounts to refetch.
//!
//! Sources of "touched":
//!
//! * every account named in a `Balances.*` or
//!   `TransactionPayment.TransactionFeePaid` event — supplied by
//!   [`crate::indexer::projections::balances::BlockBalanceProjection::touched_accounts`]
//!   which sees *every* account-bearing event, including ones we
//!   deliberately don't model as a [`BalanceMovementRow`] (e.g.
//!   `BalanceSet`, `Upgraded`). Decoupling "touched" from the movement
//!   rows is what keeps the snapshot complete for pallets using the
//!   modern `fungible::hold` API (held/released/burned_held) whose
//!   deltas we don't want to double-count in the UI activity feed;
//! * every signed extrinsic's signer, because a signed extrinsic always
//!   pays some (possibly zero) fee and advances the signer's nonce —
//!   both of which belong on the snapshot.
//!
//! The output is deterministically ordered by first touch so tests can
//! pin it and so the downstream RPC fan-out processes the same accounts
//! in the same order on every replay.

use std::collections::HashSet;

use super::extrinsics::ExtrinsicRow;

/// Collect the set of accounts touched by one block, deduplicated and
/// ordered by first appearance.
///
/// First-touch ordering keeps the RPC fan-out stable across replays and
/// makes test assertions straightforward (no sort-before-compare
/// gymnastics). Cost is a single HashSet + single Vec pass, well inside
/// the block-processing budget even for high-traffic blocks.
pub fn collect_touched(from_events: &[[u8; 32]], extrinsics: &[ExtrinsicRow]) -> Vec<[u8; 32]> {
    let mut seen: HashSet<[u8; 32]> = HashSet::new();
    let mut out: Vec<[u8; 32]> = Vec::new();

    let push = |acct: [u8; 32], seen: &mut HashSet<[u8; 32]>, out: &mut Vec<[u8; 32]>| {
        if seen.insert(acct) {
            out.push(acct);
        }
    };

    for acct in from_events {
        push(*acct, &mut seen, &mut out);
    }
    for x in extrinsics {
        if let Some(signer) = x.signer {
            push(signer, &mut seen, &mut out);
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: [u8; 32] = [0xaa; 32];
    const B: [u8; 32] = [0xbb; 32];
    const C: [u8; 32] = [0xcc; 32];

    fn ext_signed(signer: [u8; 32]) -> ExtrinsicRow {
        ExtrinsicRow {
            block_num: 1,
            idx: 0,
            hash: [0u8; 32],
            pallet: "Balances".into(),
            call: "transfer".into(),
            signer: Some(signer),
            tip: None,
            fee: 0,
            nonce: Some(0),
            success: true,
            error_module: None,
            error_name: None,
            args_scale: vec![],
        }
    }

    fn ext_unsigned() -> ExtrinsicRow {
        ExtrinsicRow {
            signer: None,
            nonce: None,
            ..ext_signed(A)
        }
    }

    /// Transfer touches both sides — the balances projection pushes
    /// `from` and `to` onto the touched list, and the collector dedups
    /// so `A` and `B` appear once each. The old pipeline got this for
    /// free via its per-account aggregate; lock the contract in the
    /// new one too.
    #[test]
    fn transfer_collects_both_sides_once() {
        let touched = collect_touched(&[A, B], &[]);
        assert_eq!(touched, vec![A, B]);
    }

    /// Single-side events (Deposit, Withdraw, Slash, Held, …) contribute
    /// exactly one account per event. Confirms the plain pass-through
    /// doesn't mangle a single-account input.
    #[test]
    fn single_side_event_collects_one_account() {
        let touched = collect_touched(&[A], &[]);
        assert_eq!(touched, vec![A]);
    }

    /// A signed extrinsic's signer must enter the touched set even when
    /// no balance events name them — sudo-paid calls and zero-fee
    /// inherent-but-signed paths would otherwise escape the snapshot,
    /// leaving their on-chain nonce adrift from the DB's copy.
    #[test]
    fn signer_without_balance_event_is_still_touched() {
        let touched = collect_touched(&[], &[ext_signed(A)]);
        assert_eq!(touched, vec![A]);
    }

    /// Unsigned (inherent) extrinsics have no signer — the collector
    /// must skip them, otherwise we'd fetch the zero-address or panic
    /// on a `None` unwrap.
    #[test]
    fn unsigned_extrinsic_contributes_nothing() {
        let touched = collect_touched(&[], &[ext_unsigned()]);
        assert!(touched.is_empty());
    }

    /// An account that signs AND shows up in a balance event must
    /// appear exactly once — the deduplication guard is the whole
    /// reason we carry a HashSet alongside the Vec.
    #[test]
    fn signer_overlapping_with_movement_dedups() {
        let touched = collect_touched(&[A], &[ext_signed(A)]);
        assert_eq!(touched, vec![A]);
    }

    /// Order is "first touch wins" — event-derived accounts are scanned
    /// before extrinsic signers, so an account that signs but also
    /// appears in a balance event keeps the event's earlier slot.
    /// Pinning this matters because the RPC fan-out consumes this slice
    /// in order; a reshuffle after the fact would change what gets
    /// fetched first on replay (stable, even if cosmetic).
    #[test]
    fn order_is_stable_by_first_touch() {
        let touched = collect_touched(&[A, B, C], &[ext_signed(C), ext_signed(A)]);
        assert_eq!(touched, vec![A, B, C]);
    }

    /// An empty block must produce an empty touched set — no snapshot
    /// RPC fan-out, no sink work. Cheap sanity check that catches any
    /// stray `push()` in the happy path.
    #[test]
    fn empty_block_produces_empty_set() {
        let touched = collect_touched(&[], &[]);
        assert!(touched.is_empty());
    }
}
