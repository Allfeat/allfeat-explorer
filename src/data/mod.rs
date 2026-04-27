//! Data layer — single seam between the explorer UI and its chain-data source.
//!
//! Everything here is server-only (`cfg(feature = "ssr")`): providers are
//! reached via server functions, never from the browser bundle. Pages keep
//! consuming [`crate::domain`] types; they don't care whether the values came
//! from the deterministic mock generator or the on-chain RPC client.
//!
//! ## Shape
//!
//! * [`provider::ChainData`] — async trait, object-safe via `async_trait`.
//! * [`mock::MockProvider`] (compile-time, behind `feature = "mock"`) — wraps
//!   [`crate::mock::data`] so the UI works end-to-end with no backend.
//! * [`rpc::RpcProvider`] (when `mock` is off) — one [`rpc::RpcClient`] per
//!   configured network; each holds a lazy subxt `OnlineClient` plus hot-TTL
//!   + finalized-LRU caches.
//! * [`error::DataError`] — uniform error surface for server functions.

pub mod cursor;
pub mod error;
pub mod filters;
pub mod provider;

#[cfg(feature = "mock")]
pub mod mock;

// `metadata` only needs subxt (already ssr-gated); having it visible in
// mock builds too lets the runtime-identity page render the
// `metadata_version` tile and the pallet-filter dropdown feed off the
// same compiled blob the RPC path decodes against.
pub mod metadata;

#[cfg(not(feature = "mock"))]
pub mod rpc;

#[cfg(not(feature = "mock"))]
pub mod indexed;

#[cfg(not(feature = "mock"))]
pub mod ss58;

pub use error::{DataError, DataResult};
pub use provider::{BoxStream, ChainData};
