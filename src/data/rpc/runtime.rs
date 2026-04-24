//! Generated runtime type bindings from on-chain metadata.
//!
//! Only the Allfeat runtime is bound today. The metadata file ships
//! types from the production Allfeat dev runtime (incl.
//! `pallet-token-allocation` and `pallet-treasury`), so both the Allfeat
//! and Melodie networks read through this single module for now.
//!
//! When the live Melodie testnet is rewired and its type shapes diverge
//! from Allfeat (e.g. it ships `MusicalWorks` / `Recordings` /
//! `Releases`), add a sibling `pub mod melodie { ... }` here with its
//! own `runtime_metadata_path = "artifacts/melodie-metadata.scale"` and
//! select per-network at the call site.
//!
//! The metadata blob lives at `artifacts/allfeat-metadata.scale`; see
//! the `subxt metadata generation scope` memory for the pallet subset
//! and the regeneration command. Paths resolve relative to
//! `CARGO_MANIFEST_DIR`.

#[subxt::subxt(runtime_metadata_path = "artifacts/allfeat-metadata.scale")]
pub mod allfeat {}
