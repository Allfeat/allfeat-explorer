//! Map subxt/runtime types to [`crate::domain`] types.
//!
//! Each public function pulls just enough data from a `OnlineClientAtBlock`
//! (header + a small set of storage queries) to build a domain value the UI
//! already knows how to render. Best-effort: when an optional field can't be
//! decoded we fall back to a sentinel rather than failing the whole request,
//! because the existing pages render a partial block (no author, zero weights)
//! gracefully and a hard error here would empty the entire list.
//!
//! Split by domain so each file stays readable:
//! * [`common`] — hash/hex formatting, SS58 coercion, block-level event index.
//! * [`blocks`] — header → `Block` and its storage reads (timestamp, weights,
//!   author).
//! * [`extrinsics`] — canonical ids and the per-block extrinsic walk.
//! * [`transfers`] — `Balances.Transfer` event projection.
//! * [`accounts`] — SS58 → `Account` lookups and the top-N scan.
//! * [`ats`] — registry walk, version resolution, feed, and aggregate stats.
//! * [`token`] — `pallet-token-allocation` envelopes, allocations, treasury.

pub mod accounts;
pub mod ats;
pub mod blocks;
pub mod common;
pub mod extrinsics;
pub mod token;
pub mod transfers;

pub use accounts::{fetch_account, fetch_top_accounts, parse_ss58};
pub use ats::{
    build_account_ats, build_account_ats_count, build_ats_feed, build_ats_list, build_ats_record,
    build_ats_stats, fetch_ats_registry_entry, fetch_ats_versions, fetch_next_ats_id,
    fetch_owner_index,
};
pub use blocks::{
    aura_slot, author_from_slot, fetch_extrinsics_summary, fetch_timestamp, fetch_weights,
    map_block,
};
pub use common::{
    account_ss58, decode_signer_ss58, fetch_block_events, hash_string, hex_bytes,
    index_events_by_phase, map_block_events, EventsByPhase,
};
pub use extrinsics::{extrinsic_id, map_extrinsics, parse_extrinsic_id};
pub use token::{build_account_allocations, build_envelope_detail, build_token_overview};
pub use transfers::{map_transfers, map_transfers_with_event_idx};
