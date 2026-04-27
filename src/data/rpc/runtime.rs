//! Generated runtime type bindings from on-chain metadata.
//!
//! Two sibling modules, one per supported runtime:
//!
//! * [`allfeat`] is built from `artifacts/allfeat-metadata.scale` — the
//!   mainnet runtime, including `pallet-token-allocation` and
//!   `pallet-treasury` for the Token hub pages.
//! * [`melodie`] is built from `artifacts/melodie-metadata.scale` — the
//!   testnet runtime, including `pallet-musical-works`,
//!   `pallet-recordings`, and `pallet-releases` (Melodie-only).
//!
//! Both modules share `SubstrateConfig` (AccountId32 / MultiAddress /
//! BlakeTwo256 / u32 block numbers) so the on-the-wire shapes overlap;
//! only the codegen `runtime_types::*` are runtime-specific. Per-network
//! call sites dispatch on
//! [`crate::network::RuntimeKind`](crate::network::RuntimeKind) carried
//! by the [`crate::data::rpc::client::RpcClient`].
//!
//! The metadata blobs live under `artifacts/`; see the
//! `subxt metadata generation scope` memory for the per-chain pallet
//! subsets and the regeneration commands. Paths resolve relative to
//! `CARGO_MANIFEST_DIR`.

#[subxt::subxt(runtime_metadata_path = "artifacts/allfeat-metadata.scale")]
pub mod allfeat {}

#[subxt::subxt(runtime_metadata_path = "artifacts/melodie-metadata.scale")]
pub mod melodie {}
