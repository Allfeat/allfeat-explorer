//! subxt-backed data provider.
//!
//! Split into:
//!   * [`cache`] — per-network hot + finalized moka caches shared by every
//!     `ChainData` method.
//!   * [`client`] — `RpcClient`: lazy `OnlineClient` + documented caching policy.
//!   * [`mappers`] — subxt runtime types → [`crate::domain`] types.
//!   * [`provider`] — [`ChainData`] impl orchestrating the two.

pub mod cache;
pub mod client;
pub mod mappers;
pub mod provider;
pub mod runtime;

pub use client::RpcClient;
pub use provider::RpcProvider;
