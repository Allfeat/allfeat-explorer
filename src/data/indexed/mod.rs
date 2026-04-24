//! `IndexedProvider` — Postgres-backed [`ChainData`] implementation.
//!
//! Serves the methods that have been migrated to the indexer, falls
//! back to the RPC provider for everything else. Phase 1 wires three
//! methods (`head_block`, `block_by_number`, `latest_blocks`); later
//! phases move more of them onto this path until the fallback is
//! retired in Phase 7 (`docs/indexing-plan.md` §7).
//!
//! The routing logic lives inside the provider itself — no separate
//! `HybridProvider` trait — so pages never have to know which backend
//! served a given query.

pub mod provider;
pub mod queries;

pub use provider::IndexedProvider;
