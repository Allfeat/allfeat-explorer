//! Domain types for the Allfeat chain explorer.
//!
//! These types are storage-agnostic: the explorer API consumes them without
//! knowing whether they came from the mock generator or the on-chain RPC
//! client. The Nuxt frontend consumes the exact same shape via the
//! ts-rs-generated bindings in `bindings/` (see `ts-bindings` feature).
//!
//! All types derive `Serialize` + `Deserialize` so axum can ship them over
//! REST and the WebSocket protocol can shuttle them live. Integer fields
//! wider than 32 bits serialise as JSON **strings** (via [`u64_string`] /
//! [`u128_string`]): JS `Number` only holds 53 significant bits, and
//! silent truncation on a balance or block number is exactly the kind of
//! bug that surfaces in production years later. `i64` timestamps stay
//! numeric — ms-since-epoch won't exceed `2^53` for a few hundred
//! millennia.

use serde::{Deserialize, Serialize};

#[cfg(feature = "ts-bindings")]
use ts_rs::TS;

/// Planck value of the reserve held on chain for every registered ATS
/// version. Source of truth for the mock generator and any UI that displays
/// total deposits.
pub const VERSION_DEPOSIT: u128 = 500_000_000_000; // 0.5 AFT

/// A substrate block.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub number: u64,
    pub hash: String,
    pub parent_hash: String,
    pub state_root: String,
    pub extrinsics_root: String,
    /// Milliseconds since Unix epoch (in the mock timeline).
    #[cfg_attr(feature = "ts-bindings", ts(type = "number"))]
    pub timestamp_ms: i64,
    pub finalized: bool,
    pub extrinsic_count: u32,
    pub event_count: u32,
    pub author: String,
    pub author_name: String,
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub ref_time: u64,
    pub ref_time_pct: u8,
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub proof_size: u64,
    pub spec_version: u32,
    pub size_bytes: u32,
}

/// Lean projection of [`Block`] for the home-page waveform hero.
///
/// The hero renders ~72 bars driven by per-block activity. Shipping the
/// full [`Block`] for each bar (~3 kB with the hashes/roots/author)
/// would burn ~250 kB on the seed payload for fields the waveform
/// doesn't read. This struct keeps only what the bar height + tooltip
/// need: counts, ref-time saturation, finalized flag, timestamp.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaveformBlock {
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub number: u64,
    pub extrinsic_count: u32,
    pub event_count: u32,
    pub ref_time_pct: u8,
    pub finalized: bool,
    #[cfg_attr(feature = "ts-bindings", ts(type = "number"))]
    pub timestamp_ms: i64,
}

impl From<&Block> for WaveformBlock {
    fn from(b: &Block) -> Self {
        Self {
            number: b.number,
            extrinsic_count: b.extrinsic_count,
            event_count: b.event_count,
            ref_time_pct: b.ref_time_pct,
            finalized: b.finalized,
            timestamp_ms: b.timestamp_ms,
        }
    }
}

impl From<Block> for WaveformBlock {
    fn from(b: Block) -> Self {
        WaveformBlock::from(&b)
    }
}

/// Execution result of an extrinsic (or dispatched call).
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CallResult {
    Success,
    Failed,
}

impl CallResult {
    pub fn as_str(self) -> &'static str {
        match self {
            CallResult::Success => "success",
            CallResult::Failed => "failed",
        }
    }
}

/// One decoded field of an event. `name` is `None` for tuple-style
/// variants (`#[codec(index)]` positional fields); `value` is the
/// `scale_value::Value` rendered via its `Display` impl — good enough
/// for the UI and stable across runtime versions.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventField {
    pub name: Option<String>,
    pub value: String,
}

/// Event emitted during extrinsic application. `fields` is empty when
/// the backend didn't decode the payload (mock, or a decode error
/// against the current metadata).
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRef {
    pub module: String,
    pub name: String,
    #[serde(default)]
    pub fields: Vec<EventField>,
}

/// Substrate block-event phase. Events with `Initialization` fire before any
/// extrinsic (e.g. `session.NewSession` on a session-change block);
/// `ApplyExtrinsic(idx)` events are the ones emitted while dispatching
/// extrinsic `idx`; `Finalization` events fire in `on_finalize`. Polkadot.js
/// displays all three — the explorer used to drop the first and last.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventPhase {
    Initialization,
    ApplyExtrinsic {
        #[cfg_attr(feature = "ts-bindings", ts(type = "number"))]
        index: u32,
    },
    Finalization,
}

/// Block-scoped event: one row per emitted event, preserving the phase so
/// the UI can render Initialization/Finalization events alongside the ones
/// attached to extrinsics.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockEvent {
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub block_number: u64,
    /// Running event index within the block (Substrate's canonical
    /// `<block>-<index>` event id uses this).
    pub index: u32,
    pub phase: EventPhase,
    pub module: String,
    pub name: String,
    #[serde(default)]
    pub fields: Vec<EventField>,
    /// Timestamp of the enclosing block. Redundant with the block's own
    /// timestamp on per-block queries, but carried here so the latest-
    /// events feed can render each row's age without a second lookup.
    #[cfg_attr(feature = "ts-bindings", ts(type = "number"))]
    pub timestamp_ms: i64,
}

/// One decoded argument of an extrinsic call. Same shape as
/// [`EventField`] plus a `type_name` sourced from the runtime metadata
/// (e.g. `"MultiAddress"`, `"Compact<Balance>"`) so the UI can label the
/// parameter without baking the pallet catalogue into the frontend.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallField {
    pub name: Option<String>,
    pub type_name: Option<String>,
    pub value: String,
}

/// Arguments of an extrinsic call. The generic [`Decoded`](Self::Decoded)
/// shape is produced by a metadata-driven walk over `args_scale`; any
/// call we can't resolve (unknown pallet / variant, truncated bytes,
/// runtime upgrade that removed the call) falls back to
/// [`Raw`](Self::Raw) so the Parameters tab still shows the payload.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExtrinsicArgs {
    Decoded { fields: Vec<CallField> },
    Raw { hex: String },
}

#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extrinsic {
    /// `"<block>-<index>"`.
    pub id: String,
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub block_number: u64,
    pub index: u32,
    pub hash: String,
    pub module: String,
    pub call: String,
    pub signed: bool,
    pub signer: Option<String>,
    pub args: ExtrinsicArgs,
    pub result: CallResult,
    pub nonce: Option<u32>,
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub tip: u128,
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub fee: u128,
    #[cfg_attr(feature = "ts-bindings", ts(type = "number"))]
    pub timestamp_ms: i64,
    pub events: Vec<EventRef>,
}

/// Balance breakdown for an account (in planck). Chain has a fixed supply
/// and no staking, so only transferable + reserved.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Balance {
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub total: u128,
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub transferable: u128,
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub reserved: u128,
}

#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub address: String,
    pub balance: Balance,
    pub nonce: u32,
    #[cfg_attr(feature = "ts-bindings", ts(type = "number"))]
    pub first_seen_ms: i64,
    #[cfg_attr(feature = "ts-bindings", ts(type = "number"))]
    pub last_active_ms: i64,
}

/// A native-asset transfer, enriched for the list views.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    pub extrinsic: Extrinsic,
    pub from: String,
    pub to: String,
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub amount: u128,
}

/// One version in an ATS registry entry.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtsVersion {
    pub version_index: u32,
    pub commitment: String,
    pub protocol_version: u8,
    #[cfg_attr(feature = "ts-bindings", ts(type = "number"))]
    pub created_at_ms: i64,
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub block_number: u64,
    pub extrinsic_index: u32,
    /// `"<block>-<index>"`.
    pub extrinsic_id: String,
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub fee: u128,
    pub signer: String,
}

#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deposit {
    pub address: String,
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub amount: u128,
}

#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtsRecord {
    pub ats_id: u32,
    pub owner: String,
    #[cfg_attr(feature = "ts-bindings", ts(type = "number"))]
    pub created_at_ms: i64,
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub created_at_block: u64,
    pub version_count: u32,
    pub deposits: Vec<Deposit>,
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub total_deposit: u128,
    pub versions: Vec<AtsVersion>,
}

impl AtsRecord {
    pub fn latest_version(&self) -> &AtsVersion {
        self.versions.last().expect("versions never empty")
    }

    pub fn initial_version(&self) -> &AtsVersion {
        &self.versions[0]
    }
}

/// Flat ATS version feed item (one row per version, sorted by time desc).
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtsFeedItem {
    pub ats_id: u32,
    pub owner: String,
    pub version_index: u32,
    pub is_initial: bool,
    pub is_latest: bool,
    pub version_count: u32,
    pub commitment: String,
    pub protocol_version: u8,
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub block_number: u64,
    pub extrinsic_id: String,
    #[cfg_attr(feature = "ts-bindings", ts(type = "number"))]
    pub timestamp_ms: i64,
    pub signer: String,
}

#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AtsStats {
    pub total: u32,
    pub total_versions: u32,
    pub last_24h: u32,
    pub last_7d: u32,
    pub last_30d: u32,
    pub unique_owners: u32,
    pub avg_per_day: u32,
    pub protocol_version: u8,
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub genesis_block: u64,
    #[cfg_attr(feature = "ts-bindings", ts(type = "number"))]
    pub first_registered_at_ms: i64,
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub total_deposited: u128,
    pub multi_version_share: f32,
}

// ── Runtime identity & upgrade history ─────────────────────────────────────
//
// Shape returned by `GET /api/v1/networks/{id}/runtime[?at=N]` and its
// `/upgrades` sibling. `RuntimeDetails` is a point-in-time snapshot of
// everything the runtime page renders without having to join data from
// half a dozen endpoints; `RuntimeUpgrade` is one row in the historical
// timeline.

/// Full `Core_version` decode (matches `sp_version::RuntimeVersion`). All
/// fields are on-chain except `metadata_version`, which is detected once
/// from the bundled metadata blob (see `data::metadata::METADATA_VERSION`).
/// `state_version` is optional because pre-V15 runtimes drop it from the
/// SCALE tail; callers should render it as "unknown" in that case rather
/// than zero.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeIdentity {
    pub spec_name: String,
    pub impl_name: String,
    pub authoring_version: u32,
    pub spec_version: u32,
    pub impl_version: u32,
    pub transaction_version: u32,
    pub state_version: Option<u8>,
}

/// `:code` blob fingerprint. Produced by reading `state_getStorage(":code")`,
/// blake2_256-hashing the raw bytes, and sniffing the first four for the
/// zstd magic (`0x28 0xB5 0x2F 0xFD`) Substrate uses to indicate a
/// compressed WASM. `size_bytes` is the length of the raw (on-chain)
/// blob, not the uncompressed WASM.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCodeInfo {
    /// `0x` + 64 hex chars.
    pub hash: String,
    pub size_bytes: u32,
    pub compressed: bool,
}

/// Everything the runtime page renders: identity, WASM fingerprint,
/// genesis / block context, and the compile-time metadata version.
///
/// When `at_block` is `None` the snapshot is at the finalized head; when
/// the caller supplied `?at=N`, the block's hash and number are copied
/// here so the UI can label the card with the exact block.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeDetails {
    pub identity: RuntimeIdentity,
    pub code: RuntimeCodeInfo,
    /// Chain genesis block hash (`chain_getBlockHash(0)`).
    pub genesis_hash: String,
    /// Block this snapshot was taken at. For the default "latest" call
    /// this is the finalized head; for `?at=N` it's the caller's `N`.
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub at_block: u64,
    pub at_block_hash: String,
    /// `V14 | V15 | V16` — the version of the compile-time metadata blob
    /// used for SCALE decoding (see `data::metadata::METADATA_VERSION`).
    pub metadata_version: u8,
}

/// One row in the `system.set_code` timeline. `first_block` is the
/// earliest block observed running this spec version — either the
/// chain's genesis (for the pre-upgrade runtime) or the block in which
/// the `system.CodeUpdated` event landed. `None` means "the backend
/// couldn't determine a deployment block" (e.g. the RPC-only fallback
/// on non-indexed networks); distinct from `Some(0)`, which is a real
/// value meaning "deployed at genesis".
///
/// `first_block_timestamp_ms` is an `Option` for the same reason and
/// also because Substrate's genesis block has no `timestamp.set`
/// inherent — the indexer stores the missing value as `0`, and
/// surfacing it as a raw epoch-ms would label genesis as "1970". The
/// DB query flips a true genesis timestamp back to `None` so the
/// frontend renders "genesis" rather than a 56-year-old date.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeUpgrade {
    pub spec_version: u32,
    #[serde(default, with = "crate::serde_helpers::u64_string_opt")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string | null"))]
    pub first_block: Option<u64>,
    #[serde(default)]
    #[cfg_attr(feature = "ts-bindings", ts(type = "number | null"))]
    pub first_block_timestamp_ms: Option<i64>,
    /// `true` for the entry whose `spec_version` matches the current
    /// runtime. Recomputed by the handler so history pages always know
    /// which row is "live".
    pub is_current: bool,
}

// ── Token allocation (pallet-token-allocation, mainnet-only) ────────────────
//
// Mirrors the on-chain shapes of `pallet-token-allocation`. Each `EnvelopeId`
// corresponds to one budget pocket of the genesis allocation; `EnvelopeInfo`
// is its config + how much of the cap has been distributed; `Allocation` is a
// single per-beneficiary entry with its current vesting state already
// resolved against the chain head (so the UI doesn't have to redo the math).

/// Budget pockets defined at genesis. Wire form is lowercase strings the
/// frontend uses verbatim in `/token/envelopes/:id` URLs.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EnvelopeId {
    #[serde(rename = "teams")]
    Teams,
    #[serde(rename = "kol")]
    KoL,
    #[serde(rename = "private1")]
    Private1,
    #[serde(rename = "private2")]
    Private2,
    #[serde(rename = "public1")]
    Public1,
    #[serde(rename = "public2")]
    Public2,
    #[serde(rename = "public3")]
    Public3,
    #[serde(rename = "public4")]
    Public4,
    #[serde(rename = "airdrop")]
    Airdrop,
    #[serde(rename = "community-rewards")]
    CommunityRewards,
    #[serde(rename = "listing")]
    Listing,
    #[serde(rename = "research-development")]
    ResearchDevelopment,
    #[serde(rename = "reserve")]
    Reserve,
}

impl EnvelopeId {
    /// Wire / URL slug for this envelope (matches the `serde(rename)` above).
    pub fn slug(self) -> &'static str {
        match self {
            EnvelopeId::Teams => "teams",
            EnvelopeId::KoL => "kol",
            EnvelopeId::Private1 => "private1",
            EnvelopeId::Private2 => "private2",
            EnvelopeId::Public1 => "public1",
            EnvelopeId::Public2 => "public2",
            EnvelopeId::Public3 => "public3",
            EnvelopeId::Public4 => "public4",
            EnvelopeId::Airdrop => "airdrop",
            EnvelopeId::CommunityRewards => "community-rewards",
            EnvelopeId::Listing => "listing",
            EnvelopeId::ResearchDevelopment => "research-development",
            EnvelopeId::Reserve => "reserve",
        }
    }

    pub fn from_slug(s: &str) -> Option<Self> {
        Some(match s {
            "teams" => EnvelopeId::Teams,
            "kol" => EnvelopeId::KoL,
            "private1" => EnvelopeId::Private1,
            "private2" => EnvelopeId::Private2,
            "public1" => EnvelopeId::Public1,
            "public2" => EnvelopeId::Public2,
            "public3" => EnvelopeId::Public3,
            "public4" => EnvelopeId::Public4,
            "airdrop" => EnvelopeId::Airdrop,
            "community-rewards" => EnvelopeId::CommunityRewards,
            "listing" => EnvelopeId::Listing,
            "research-development" => EnvelopeId::ResearchDevelopment,
            "reserve" => EnvelopeId::Reserve,
            _ => return None,
        })
    }

    /// Human-readable label for UI.
    pub fn label(self) -> &'static str {
        match self {
            EnvelopeId::Teams => "Teams",
            EnvelopeId::KoL => "KoL",
            EnvelopeId::Private1 => "Private Sale 1",
            EnvelopeId::Private2 => "Private Sale 2",
            EnvelopeId::Public1 => "Public Sale 1",
            EnvelopeId::Public2 => "Public Sale 2",
            EnvelopeId::Public3 => "Public Sale 3",
            EnvelopeId::Public4 => "Public Sale 4",
            EnvelopeId::Airdrop => "Airdrop",
            EnvelopeId::CommunityRewards => "Community Rewards",
            EnvelopeId::Listing => "Listing",
            EnvelopeId::ResearchDevelopment => "Research & Development",
            EnvelopeId::Reserve => "Reserve",
        }
    }

    pub const ALL: [EnvelopeId; 13] = [
        EnvelopeId::Teams,
        EnvelopeId::KoL,
        EnvelopeId::Private1,
        EnvelopeId::Private2,
        EnvelopeId::Public1,
        EnvelopeId::Public2,
        EnvelopeId::Public3,
        EnvelopeId::Public4,
        EnvelopeId::Airdrop,
        EnvelopeId::CommunityRewards,
        EnvelopeId::Listing,
        EnvelopeId::ResearchDevelopment,
        EnvelopeId::Reserve,
    ];

    /// SCALE variant index — must match `#[codec(index = N)]` on the on-chain
    /// enum so the mapper's sub-account derivation stays stable across runtime
    /// upgrades. The mock generator routes through the same accessor to seed
    /// its pot addresses, guaranteeing both backends agree on per-envelope
    /// indices without relying on the enum's discriminant cast.
    pub fn variant_index(self) -> u8 {
        match self {
            EnvelopeId::Teams => 0,
            EnvelopeId::KoL => 1,
            EnvelopeId::Private1 => 2,
            EnvelopeId::Private2 => 3,
            EnvelopeId::Public1 => 4,
            EnvelopeId::Public2 => 5,
            EnvelopeId::Public3 => 6,
            EnvelopeId::Public4 => 7,
            EnvelopeId::Airdrop => 8,
            EnvelopeId::CommunityRewards => 9,
            EnvelopeId::Listing => 10,
            EnvelopeId::ResearchDevelopment => 11,
            EnvelopeId::Reserve => 12,
        }
    }
}

/// One genesis envelope: cap, vesting params, distribution progress.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvelopeInfo {
    pub id: EnvelopeId,
    pub label: String,
    /// Pallet sub-account holding the envelope balance.
    pub account: String,
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub total_cap: u128,
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub distributed: u128,
    /// 0..=100 percentage paid up front at allocation time.
    pub upfront_pct: u8,
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub cliff_blocks: u64,
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub vesting_duration_blocks: u64,
    pub unique_beneficiary: Option<String>,
    /// Number of allocations issued from this envelope.
    pub allocation_count: u32,
}

/// One per-beneficiary allocation, with its claimable amount resolved at the
/// chain head (so the UI doesn't recompute vesting math on every render).
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allocation {
    pub id: u32,
    pub envelope: EnvelopeId,
    pub beneficiary: String,
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub total: u128,
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub upfront: u128,
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub vested_total: u128,
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub released: u128,
    /// Block at which vesting starts (= max(allocation.start, envelope.cliff)).
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub start_block: u64,
    /// Vested amount currently claimable at the chain head (post-released).
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub claimable_now: u128,
    /// 0..=100 share of `vested_total` already released.
    pub percent_vested: u8,
}

/// Vested-but-unreleased amount at `head_block`. Mirrors
/// `pallet-token-allocation::claimable_amount`: linear release between
/// `start_block` and `start_block + vesting_duration`, capped at
/// `vested_total - released`. Both backends (mock generator and RPC mapper)
/// route through this single implementation so the UI's "% vested" progression
/// matches on-chain math byte-for-byte.
pub fn claimable_amount(
    head_block: u64,
    start_block: u64,
    vested_total: u128,
    released: u128,
    vesting_duration: u64,
) -> u128 {
    if head_block <= start_block {
        return 0;
    }
    let elapsed = head_block - start_block;
    if vesting_duration == 0 || elapsed >= vesting_duration {
        return vested_total.saturating_sub(released);
    }
    let vested_now = vested_total
        .saturating_mul(elapsed as u128)
        .saturating_div(vesting_duration as u128);
    vested_now.saturating_sub(released)
}

/// Full detail for a single envelope: config + allocation list.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvelopeDetail {
    pub envelope: EnvelopeInfo,
    pub allocations: Vec<Allocation>,
}

/// Token-wide snapshot: balances + epoch state + per-envelope summary.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenOverview {
    pub symbol: String,
    pub decimals: u8,
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub total_supply: u128,
    /// Free-balance share of supply (= total − locked − envelope reserves).
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub circulating: u128,
    /// Sum of `held` balances across all per-beneficiary holds (= still
    /// vesting). Mirrors the pallet's `HoldReason::TokenAllocation`.
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub locked: u128,
    /// Sum of cap remaining inside envelope sub-accounts (undistributed).
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub envelope_reserves: u128,
    pub treasury: TreasuryInfo,
    pub epoch: EpochInfo,
    pub envelopes: Vec<EnvelopeInfo>,
}

#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreasuryInfo {
    pub account: String,
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub balance: u128,
    /// Held-under-vesting amount owed to the treasury account (subset of
    /// [`TokenOverview::locked`] whose beneficiary is the treasury pot).
    #[serde(with = "crate::serde_helpers::u128_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub locked: u128,
}

#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpochInfo {
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub index: u64,
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub head_block: u64,
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub next_payout_block: u64,
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub epoch_duration_blocks: u64,
}

// ── Pagination envelope ─────────────────────────────────────────────────────
//
// Unified response shape for every list endpoint under `/api/v1/`. The design
// is documented in `docs/api-pagination-plan.md`; the short version is:
//
// - Cursor-based, never offset-based — stable under concurrent writes.
// - Cursors are transparent strings (e.g. `12345-3`) whose grammar is owned
//   by `src/data/cursor.rs`. The client round-trips them verbatim.
// - `has_more` comes from a fetch-N+1 trick at the query layer, so there's
//   no extra `COUNT(*)`.
// - `total` is opportunistic: present when the provider already has a cheap
//   source of truth (chain head, `ats_stats`, small table counts), `None`
//   otherwise.

/// Paginated list response. Every `/api/v1/` list endpoint returns this
/// shape so the frontend binds once (see `usePaginatedList`).
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page_info: PageInfo,
}

impl<T> Page<T> {
    pub fn empty() -> Self {
        Self {
            items: Vec::new(),
            page_info: PageInfo::default(),
        }
    }
}

/// Metadata attached to a [`Page<T>`]. `has_more` is always accurate
/// (derived from the fetch-N+1 trick); `next_cursor` is `None` on the
/// last page; `total` is populated only when cheap to compute.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageInfo {
    /// Overall row count for the query (filters applied). `None` when the
    /// backend has no cheap source for it — callers must not assume
    /// `total` reflects the number of items currently loaded.
    #[serde(default, with = "crate::serde_helpers::u64_string_opt")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string | null"))]
    pub total: Option<u64>,
    /// Opaque (to the client) cursor to pass back as `?cursor=…` for the
    /// next page. `None` iff `has_more == false`.
    pub next_cursor: Option<String>,
    /// True iff at least one more row exists beyond the current page.
    pub has_more: bool,
}

/// Query-param shape shared by every paginated endpoint. Handlers extract
/// it via `axum::extract::Query`; the frontend mirrors it through the
/// `usePaginatedList` composable so both ends agree on field names.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRequest {
    pub count: u32,
    #[serde(default)]
    pub cursor: Option<String>,
}
