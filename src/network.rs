//! Per-network mock chain specification.
//!
//! Each chain has a `genesis_ms` — a fixed past wall-clock instant the chain
//! was "born". Head block, block timestamps, and ATS counts are all pure
//! functions of `(now_ms, spec)`. There are no frozen "now" constants: SSR
//! reads system time, the client reads `Date.now()`, and both tick forward
//! from there during the live mock run.

use serde::Serialize;

#[cfg(feature = "ts-bindings")]
use ts_rs::TS;

/// Discriminator for the codegen runtime module a [`NetworkSpec`] reads
/// through. Picked up by [`crate::data::rpc::client::RpcClient`] at
/// construction so every per-network call site can dispatch on it
/// (`match runtime_kind { Allfeat => runtime::allfeat::…, Melodie =>
/// runtime::melodie::… }`). Both runtimes share `SubstrateConfig`, so
/// the connection itself is uniform — only the codegen `runtime_types`
/// differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeKind {
    Allfeat,
    Melodie,
}

/// Static, compile-time description of a mock chain.
///
/// `Serialize` is derived so the `/api/v1/networks` handler can emit
/// the catalogue directly; `&'static str` fields serialise as strings.
/// No `Deserialize` on purpose — the wire contract is one-way and the
/// server never ingests this shape from a client. The internal mock
/// generator fields (seed, genesis, authors, ats_blocks_per) are
/// skipped on both sides so they never leak into the TS bindings or
/// the JSON response.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
pub struct NetworkSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: &'static str,
    pub testnet: bool,
    pub token: &'static str,
    #[serde(serialize_with = "crate::serde_helpers::u64_string::serialize")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub block_time_secs: u64,
    pub spec_version: u32,
    /// Runtime's on-chain `spec_name` (from `Core_version`). Static here so
    /// mock builds and first-paint SSR have a value; the `/networks` handler
    /// overrides it with the live value when RPC is connected.
    pub spec_name: &'static str,
    /// SS58 address-format prefix (`ss58Format` in `system_properties`). Static
    /// here so mock builds and first-paint SSR can encode addresses without
    /// reaching the node; the `/networks` handler overrides it with the live
    /// value when RPC is connected, and every backend address-encoding path
    /// reads the live value via the per-client cache so on-disk DB rows
    /// (stored as raw 32-byte keys) surface under the correct prefix per
    /// network. 42 is the generic-Substrate default (addresses starting with
    /// `5`); override per chain once the authoritative value is known.
    pub ss58_prefix: u16,
    /// Mixed into every deterministic generator so output diverges per network.
    #[serde(skip)]
    #[cfg_attr(feature = "ts-bindings", ts(skip))]
    pub seed: u32,
    /// Wall-clock instant (ms since epoch) of block 0. Picks a date a few
    /// months back so the head looks "lived-in" at boot.
    #[serde(skip)]
    #[cfg_attr(feature = "ts-bindings", ts(skip))]
    pub genesis_ms: i64,
    #[serde(skip)]
    #[cfg_attr(feature = "ts-bindings", ts(skip))]
    pub authors: &'static [&'static str],
    /// One new ATS work is registered every `ats_blocks_per` blocks. Lower
    /// numbers make the live timeline visibly stream new entries.
    #[serde(skip)]
    #[cfg_attr(feature = "ts-bindings", ts(skip))]
    pub ats_blocks_per: u32,
    pub endpoint: &'static str,
    /// Codegen runtime module this network reads through. Backend-only
    /// detail — never crosses the wire, never appears in TS bindings.
    /// Hand-fixed at the spec declaration; the indexer and every RPC
    /// mapper dispatches on the value carried by the per-network
    /// [`crate::data::rpc::client::RpcClient`].
    #[serde(skip)]
    #[cfg_attr(feature = "ts-bindings", ts(skip))]
    pub runtime_kind: RuntimeKind,
}

// Genesis instants, picked once so per-network head/ATS counts at boot land
// in a "lived-in" range. The exact dates are arbitrary — what matters is the
// elapsed time relative to the user's wall clock.
const ALLFEAT_GENESIS_MS: i64 = 1_763_513_190_000; // 2025-11-19T12:46:30Z (~5 months back)
const MELODIE_GENESIS_MS: i64 = 1_774_761_990_000; // 2026-03-29T12:46:30Z (~3 weeks back)

pub const ALLFEAT: NetworkSpec = NetworkSpec {
    id: "allfeat",
    name: "Allfeat",
    kind: "Mainnet",
    testnet: false,
    token: "AFT",
    block_time_secs: 6,
    spec_version: 1_001_004,
    spec_name: "allfeat",
    // Allfeat mainnet is registered at `ss58Format = 440` — every address
    // starts with `q`. Set as the static default so mock builds, first-paint
    // SSR, and the RPC-fallback path all render the correct prefix without
    // depending on `system_properties` always being reachable; the
    // `/networks` handler still overrides with the live value from the node
    // when the chain actually publishes one.
    ss58_prefix: 440,
    seed: 0x0A1F_EA70,
    genesis_ms: ALLFEAT_GENESIS_MS,
    authors: &[
        "Allfeat-1",
        "Allfeat-2",
        "Allfeat-3",
        "Allfeat-4",
        "Allfeat-5",
        "Allfeat-6",
        "Allfeat-7",
        "Allfeat-8",
    ],
    ats_blocks_per: 5,
    endpoint: "wss://mainnet.rpc.allfeat.org",
    runtime_kind: RuntimeKind::Allfeat,
};

pub const MELODIE: NetworkSpec = NetworkSpec {
    id: "melodie",
    name: "Melodie",
    kind: "Testnet",
    testnet: true,
    token: "MEL",
    block_time_secs: 3,
    spec_version: 1_002_011,
    spec_name: "melodie",
    // Generic-Substrate prefix (addresses starting with `5`). Melodie is a
    // testnet and keeps the default `ss58Format = 42` — only the mainnet
    // runtime registers the Allfeat-specific 440. Keeping the prefixes
    // distinct per kind avoids the "looks like mainnet" confusion on
    // Melodie-facing deployments.
    ss58_prefix: 42,
    seed: 0xBEAA_F00D,
    genesis_ms: MELODIE_GENESIS_MS,
    authors: &[
        "Melodie-1",
        "Melodie-2",
        "Melodie-3",
        "Melodie-4",
        "Melodie-5",
        "Melodie-6",
        "Melodie-7",
        "Melodie-8",
    ],
    ats_blocks_per: 3,
    endpoint: "wss://melodie-rpc.allfeat.io",
    runtime_kind: RuntimeKind::Melodie,
};

pub const NETWORKS: &[&NetworkSpec] = &[&ALLFEAT, &MELODIE];
pub const DEFAULT_NETWORK: &NetworkSpec = &ALLFEAT;

pub fn by_id(id: &str) -> Option<&'static NetworkSpec> {
    NETWORKS.iter().copied().find(|n| n.id == id)
}

pub fn by_id_or_default(id: &str) -> &'static NetworkSpec {
    by_id(id).unwrap_or(DEFAULT_NETWORK)
}

/// Live chain context: a network spec + the wall-clock "now" the caller is
/// rendering against. SSR uses system time; the client uses `Date.now()` and
/// ticks it forward in `block_time_secs`-sized steps after hydration.
///
/// All chain quantities (head block, block timestamps, ATS counts) are pure
/// functions of `(now_ms, spec)` — no frozen baseline.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ChainCtx {
    pub spec: &'static NetworkSpec,
    pub now_ms: i64,
}

impl ChainCtx {
    pub fn new(spec: &'static NetworkSpec, now_ms: i64) -> Self {
        Self { spec, now_ms }
    }

    /// Block currently at the head: blocks elapsed since genesis at the
    /// network's cadence.
    pub fn head_block(&self) -> u64 {
        let elapsed_ms = (self.now_ms - self.spec.genesis_ms).max(0);
        let bt = self.spec.block_time_secs.max(1) as i64;
        ((elapsed_ms / 1000) as u64) / (bt as u64)
    }

    /// Deterministic timestamp for an arbitrary block — purely a function of
    /// genesis + block number.
    pub fn block_timestamp_ms(&self, num: u64) -> i64 {
        self.spec.genesis_ms + (num as i64) * (self.spec.block_time_secs as i64) * 1000
    }

    /// Total ATS works registered, derived from blocks elapsed since genesis.
    pub fn ats_total(&self) -> u32 {
        let head = self.head_block();
        let per = self.spec.ats_blocks_per.max(1) as u64;
        ((head / per) + 1) as u32
    }

    /// Total ATS versions — slightly more than works (some are revised).
    pub fn ats_version_total(&self) -> u32 {
        ((self.ats_total() as u64) * 3 / 2) as u32
    }
}
