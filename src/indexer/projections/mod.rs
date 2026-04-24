//! Projections — pure `(subxt view) → Row` transforms.
//!
//! A projection never talks to the database. It takes a
//! `OnlineClientAtBlock<SubstrateConfig>` (which already carries the
//! metadata for its block's spec_version) plus whatever it needs from
//! the chain, and produces a `Row` struct ready for the sink.
//!
//! Keeping the transforms pure means:
//!
//! * the live worker and the (Phase 2) backfill worker share the exact
//!   same projection code — no "live-only" vs "backfill-only" drift;
//! * unit tests exercise every branch on in-memory SCALE fixtures —
//!   no Postgres required to validate decode logic;
//! * adding a new table = adding a new projection + a new sink entry,
//!   with zero coupling to the workers above.
//!
//! Phase 1 only implements [`blocks`]. The other submodules exist as
//! stubs so later phases slot in without churning the module graph.

pub mod accounts;
pub mod ats;
pub mod balances;
pub mod blocks;
pub mod events;
pub mod extrinsics;
pub mod genesis;
