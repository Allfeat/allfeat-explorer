//! Mock chain-data generator.
//!
//! Whole module is gated behind `feature = "mock"` so production builds never
//! link the synthetic generator. When the feature is on, [`crate::data::mock`]
//! wraps this module's generators to satisfy [`crate::data::ChainData`] and
//! the subxt RPC path is excluded entirely — the explorer behaves as if the
//! on-chain backend weren't implemented yet.
//!
//! Generators are deterministic and parameterized by
//! [`crate::network::ChainCtx`] (network spec + wall-clock `now_ms`). Two
//! networks at the same input never collide, and the live head ticks forward
//! as `now_ms` advances.

pub mod data;
pub mod rng;

pub use data::{
    build_ats, build_block, build_extrinsics, get_account, get_account_ats, get_account_ats_count,
    get_ats_list, get_ats_stats, get_ats_version_feed, get_blocks, get_latest_extrinsics,
    get_top_accounts, get_transfers, hex_seeded, ss58_seeded,
};
