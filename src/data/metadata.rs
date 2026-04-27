//! Per-network static runtime metadata and dynamic event/call decoders.
//!
//! The event projection stores `data_scale` (raw field bytes) rather
//! than a pre-decoded JSON blob — see
//! [`crate::indexer::projections::events::EventRow`]. Turning those
//! bytes back into human-readable values is a read-time concern, and
//! it needs:
//!
//! 1. A `subxt::Metadata` (for pallet / variant / field type lookup
//!    against the `scale_info::PortableRegistry`).
//! 2. `scale_value::scale::decode_as_fields` to walk the bytes into a
//!    `Composite<TypeId>`.
//!
//! Each supported runtime ships its own metadata blob (see
//! [`ALLFEAT_RUNTIME`] and [`MELODIE_RUNTIME`]) — same files the
//! `#[subxt::subxt]` macros consume at compile time. The decoded
//! [`subxt::Metadata`] is kept behind a [`LazyLock`] per runtime so the
//! decode cost is paid at most once per process per network, on first
//! read. Callers reach the right blob through [`runtime_for`], passing
//! the `network_id` they already have on hand.

use std::sync::LazyLock;

use subxt::Metadata;

// The SCALE-decoding helpers below exist only on the non-mock (RPC /
// indexed) code paths — they lean on `data::rpc::mappers` and `data::ss58`,
// both gated to `not(mock)`. The top-level metadata surface
// (`metadata_version_for`, `callable_pallet_names`) is pure-reads of the
// compile-time blobs and stays available in every build so the frontend's
// runtime-identity tile and pallet-filter dropdown don't care which
// backend feeds them.
#[cfg(not(feature = "mock"))]
use subxt::ext::{scale_decode, scale_value};

#[cfg(not(feature = "mock"))]
use crate::data::rpc::mappers::hex_bytes;
#[cfg(not(feature = "mock"))]
use crate::domain::{CallField, EventField, ExtrinsicArgs};
use crate::network::{by_id_or_default, RuntimeKind};

/// Compile-time metadata blob for the Allfeat mainnet runtime. Same file
/// the generated `runtime_types` module is built from, so decoding here
/// and static binding in [`crate::data::rpc::runtime::allfeat`] stay in
/// lockstep.
const ALLFEAT_METADATA_BLOB: &[u8] = include_bytes!("../../artifacts/allfeat-metadata.scale");

/// Compile-time metadata blob for the Melodie testnet runtime. Pairs
/// with [`crate::data::rpc::runtime::melodie`].
const MELODIE_METADATA_BLOB: &[u8] = include_bytes!("../../artifacts/melodie-metadata.scale");

/// Per-runtime bundle: the raw SCALE bytes (for the
/// `/api/v1/networks/{id}/runtime/metadata` download endpoint), the
/// declared metadata version (for the runtime-identity tile), and a
/// lazy [`Metadata`] for SCALE decoding.
///
/// Uses a `fn()` initialiser rather than a closure so the type stays
/// `LazyLock<Metadata>` (the default `F = fn() -> T`) and can live in a
/// `static`.
pub struct RuntimeMetadata {
    pub bytes: &'static [u8],
    pub version: u8,
    pub decoded: LazyLock<Metadata>,
}

fn decode_allfeat_metadata() -> Metadata {
    Metadata::decode_from(ALLFEAT_METADATA_BLOB).expect("allfeat metadata decodes")
}

fn decode_melodie_metadata() -> Metadata {
    Metadata::decode_from(MELODIE_METADATA_BLOB).expect("melodie metadata decodes")
}

pub static ALLFEAT_RUNTIME: RuntimeMetadata = RuntimeMetadata {
    bytes: ALLFEAT_METADATA_BLOB,
    version: detect_metadata_version(ALLFEAT_METADATA_BLOB),
    decoded: LazyLock::new(decode_allfeat_metadata),
};

pub static MELODIE_RUNTIME: RuntimeMetadata = RuntimeMetadata {
    bytes: MELODIE_METADATA_BLOB,
    version: detect_metadata_version(MELODIE_METADATA_BLOB),
    decoded: LazyLock::new(decode_melodie_metadata),
};

/// Resolve the metadata bundle for a network by id. Falls back to the
/// default network's bundle if `network_id` doesn't match a known spec —
/// matches `crate::network::by_id_or_default`'s contract so the rest of
/// the read path stays total.
pub fn runtime_for(network_id: &str) -> &'static RuntimeMetadata {
    match by_id_or_default(network_id).runtime_kind {
        RuntimeKind::Allfeat => &ALLFEAT_RUNTIME,
        RuntimeKind::Melodie => &MELODIE_RUNTIME,
    }
}

/// Raw SCALE-encoded metadata bytes for `network_id`. Feeds the
/// `/api/v1/networks/{id}/runtime/metadata` download so the frontend's
/// "Raw metadata" button returns the exact same artifact the server's
/// decoders consume — guarantees what the user downloads matches what
/// the explorer renders.
pub fn metadata_bytes_for(network_id: &str) -> &'static [u8] {
    runtime_for(network_id).bytes
}

/// Runtime metadata version declared by the SCALE prefix of the bundled
/// blob for `network_id`. Exposed via `/api/v1/networks/{id}/runtime` so
/// the runtime page doesn't have to hardcode `"V15"`.
pub fn metadata_version_for(network_id: &str) -> u8 {
    runtime_for(network_id).version
}

/// Read directly off the SCALE prefix: the file is a
/// `frame_metadata::RuntimeMetadataPrefixed` whose first four bytes are
/// the magic `"meta"` and whose fifth byte is the SCALE variant index
/// of the inner `RuntimeMetadata` enum — V0 → 0, … V14 → 14, V15 → 15,
/// V16 → 16. Subxt's `Metadata` type collapses V14/V15/V16 into a single
/// decoded shape, so we can't recover the version number from there
/// after `decode_from`.
const fn detect_metadata_version(blob: &[u8]) -> u8 {
    // Prefix: `"meta" || variant_index (1 byte) || payload`. A blob that
    // doesn't match the magic isn't consumable anyway (subxt's decode
    // below would panic), so we surface a sentinel `0` rather than loop
    // the detection logic.
    if blob.len() < 5 || blob[0] != b'm' || blob[1] != b'e' || blob[2] != b't' || blob[3] != b'a' {
        return 0;
    }
    blob[4]
}

/// All pallet names in `network_id`'s runtime that expose at least one
/// callable extrinsic, returned in metadata-declaration order. The UI's
/// extrinsics filter dropdown reads this to stay in lockstep with the
/// `extrinsics.pallet` column the indexer writes — filtering pallets
/// without `call_variants()` would just show an empty page, and
/// including them would mislead the user.
pub fn callable_pallet_names(network_id: &str) -> Vec<String> {
    runtime_for(network_id)
        .decoded
        .pallets()
        .filter_map(|p| {
            let has_calls = p.call_variants().map(|v| !v.is_empty()).unwrap_or(false);
            has_calls.then(|| p.name().to_string())
        })
        .collect()
}

/// Decode the raw field bytes of an event into a list of
/// `(name?, value)` pairs using the static metadata for `network_id`.
///
/// Returns an empty vec on any lookup or decode failure — callers
/// surface "no decoded fields" the same way they already surface
/// events that genuinely carry no payload. Swallowing the error here
/// keeps the query path total: one malformed event must not poison
/// the whole extrinsic read.
#[cfg(not(feature = "mock"))]
pub fn decode_event_fields(
    network_id: &str,
    pallet_name: &str,
    variant_name: &str,
    data: &[u8],
    ss58_prefix: u16,
) -> Vec<EventField> {
    let metadata = &*runtime_for(network_id).decoded;
    let Some(pallet) = metadata.pallet_by_name(pallet_name) else {
        return Vec::new();
    };
    let Some(variant) = pallet
        .event_variants()
        .and_then(|variants| variants.iter().find(|v| v.name == variant_name))
    else {
        return Vec::new();
    };

    let mut fields_iter = variant
        .fields
        .iter()
        .map(|f| scale_decode::Field::new(f.ty.id, f.name.as_deref()));

    let mut cursor = data;
    let composite =
        match scale_value::scale::decode_as_fields(&mut cursor, &mut fields_iter, metadata.types())
        {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

    match composite {
        scale_value::Composite::Named(named) => named
            .into_iter()
            .map(|(name, value)| EventField {
                name: Some(name),
                value: render_value(&value, ss58_prefix),
            })
            .collect(),
        scale_value::Composite::Unnamed(unnamed) => unnamed
            .into_iter()
            .map(|value| EventField {
                name: None,
                value: render_value(&value, ss58_prefix),
            })
            .collect(),
    }
}

/// Decode the SCALE-encoded `call_data_bytes()` shape subxt produces —
/// `[pallet_idx, call_idx, …field_bytes]` — into the domain
/// [`ExtrinsicArgs`] variants, using `network_id`'s runtime metadata.
///
/// Both pipes into this function carry the 2-byte pallet/call prefix:
///
/// * The live RPC mapper passes `ext.call_data_bytes()` directly.
/// * The indexer stores that same buffer into `extrinsics.args_scale`
///   (see [`crate::indexer::projections::extrinsics::map`]).
///
/// The pallet/call *names* are already resolved by the caller, so the
/// prefix bytes are redundant here — we strip them before feeding the
/// remainder to [`decode_call_fields`]. Anything shorter than 2 bytes
/// or that decodes to no fields falls back to
/// [`ExtrinsicArgs::Raw`](crate::domain::ExtrinsicArgs::Raw) so the
/// Parameters tab still shows something.
#[cfg(not(feature = "mock"))]
pub fn decode_call_args(
    network_id: &str,
    pallet_name: &str,
    call_name: &str,
    data: &[u8],
    ss58_prefix: u16,
) -> ExtrinsicArgs {
    let raw = || ExtrinsicArgs::Raw {
        hex: hex_bytes(data),
    };
    // The stored buffer always begins with `[pallet_idx, call_idx]`.
    // If it's missing we can't meaningfully decode args — surface the
    // opaque hex rather than risk a mis-aligned field walk.
    let Some(args_only) = data.get(2..) else {
        return raw();
    };
    let fields = decode_call_fields(network_id, pallet_name, call_name, args_only, ss58_prefix);
    if fields.is_empty() && !args_only.is_empty() {
        raw()
    } else {
        ExtrinsicArgs::Decoded { fields }
    }
}

/// Decode the field-level bytes of an extrinsic call (the portion
/// *after* the 2-byte pallet/call prefix) into a list of
/// `(name?, type_name?, value)` tuples, using `network_id`'s runtime
/// metadata. Mirrors [`decode_event_fields`] — the only structural
/// difference is `call_variant_by_name` instead of walking event
/// variants.
///
/// Returns an empty vec on any lookup or decode failure so the caller
/// can fall back uniformly to [`ExtrinsicArgs::Raw`]. Callers that
/// start from subxt's `call_data_bytes()` shape should go through
/// [`decode_call_args`] instead.
#[cfg(not(feature = "mock"))]
pub fn decode_call_fields(
    network_id: &str,
    pallet_name: &str,
    call_name: &str,
    data: &[u8],
    ss58_prefix: u16,
) -> Vec<CallField> {
    let metadata = &*runtime_for(network_id).decoded;
    let Some(pallet) = metadata.pallet_by_name(pallet_name) else {
        return Vec::new();
    };
    let Some(variant) = pallet.call_variant_by_name(call_name) else {
        return Vec::new();
    };

    // `type_name` carries the runtime's human label for the argument
    // (e.g. `"AccountIdLookupOf<T>"`, `"Compact<T::Balance>"`). Captured
    // alongside the decoded value so the UI can render the parameter
    // type without hard-coding a pallet catalogue.
    let type_names: Vec<Option<String>> =
        variant.fields.iter().map(|f| f.type_name.clone()).collect();

    let mut fields_iter = variant
        .fields
        .iter()
        .map(|f| scale_decode::Field::new(f.ty.id, f.name.as_deref()));

    let mut cursor = data;
    let composite =
        match scale_value::scale::decode_as_fields(&mut cursor, &mut fields_iter, metadata.types())
        {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

    match composite {
        scale_value::Composite::Named(named) => named
            .into_iter()
            .enumerate()
            .map(|(i, (name, value))| CallField {
                name: Some(name),
                type_name: type_names.get(i).cloned().flatten(),
                value: render_value(&value, ss58_prefix),
            })
            .collect(),
        scale_value::Composite::Unnamed(unnamed) => unnamed
            .into_iter()
            .enumerate()
            .map(|(i, value)| CallField {
                name: None,
                type_name: type_names.get(i).cloned().flatten(),
                value: render_value(&value, ss58_prefix),
            })
            .collect(),
    }
}

/// Render a decoded `scale_value::Value` into a compact human-readable
/// string. The default `Value::Display` produces `Id (((86, 210, …)))`
/// for a `MultiAddress::Id(AccountId32)` — unusable in the UI. Two
/// narrow special cases cover the vast majority of Substrate extrinsic
/// arguments:
///
/// * **32-byte unnamed composite of `u8` primitives** → SS58-encode.
///   In extrinsic arg position this is almost always an account id
///   (`AccountId32`, `H256` used as account key, etc.); the occasional
///   false positive (a 32-byte hash) is still a valid SS58 string and
///   costs nothing semantically — the `type_name` alongside the value
///   already tells the reader what the type is.
/// * **Single-field unnamed `Variant`** (e.g. `MultiAddress::Id(inner)`,
///   `Option::Some(inner)`) → render the inner value; the tag is
///   redundant with the parent field's `type_name`.
///
/// Everything else falls back to the scale-value `Display` impl, which
/// is already correct for primitives (u128 → `"123"`, bool → `"true"`,
/// etc.) and only ugly for the nested-composite cases above.
#[cfg(not(feature = "mock"))]
fn render_value(value: &scale_value::Value<u32>, ss58_prefix: u16) -> String {
    if let Some(bytes) = try_extract_32_bytes(value) {
        return crate::data::ss58::encode_ss58(&bytes, ss58_prefix);
    }

    if let scale_value::ValueDef::Variant(v) = &value.value {
        if let scale_value::Composite::Unnamed(inner) = &v.values {
            if inner.len() == 1 {
                return render_value(&inner[0], ss58_prefix);
            }
        }
    }

    value.to_string()
}

/// Walk a `Value` down the usual newtype / wrapper shapes substrate
/// uses for a 32-byte id and return the bytes when we find them.
///
/// Handles:
///
/// * `Primitive(U256(bytes))` — rare but possible.
/// * `Composite::Unnamed([u8; 32])` — bare `AccountId32([u8;32])`, also
///   `H256` and similar fixed-width hashes.
/// * `Composite::Unnamed([single_inner])` — newtype wrappers that hold
///   a single nested composite (how scale-value encodes a tuple-struct
///   with one field, e.g. `AccountId32(pub [u8; 32])`).
#[cfg(not(feature = "mock"))]
fn try_extract_32_bytes(value: &scale_value::Value<u32>) -> Option<[u8; 32]> {
    match &value.value {
        scale_value::ValueDef::Primitive(scale_value::Primitive::U256(b)) => Some(*b),
        scale_value::ValueDef::Composite(scale_value::Composite::Unnamed(items)) => {
            if items.len() == 32 {
                let mut out = [0u8; 32];
                for (i, item) in items.iter().enumerate() {
                    match &item.value {
                        scale_value::ValueDef::Primitive(scale_value::Primitive::U128(n))
                            if *n <= u8::MAX as u128 =>
                        {
                            out[i] = *n as u8;
                        }
                        _ => return None,
                    }
                }
                Some(out)
            } else if items.len() == 1 {
                // Newtype wrapper: dig one level in.
                try_extract_32_bytes(&items[0])
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both bundled blobs ship with the binary — loading them can't
    /// fail, and the hit path depends on this being a valid
    /// `Metadata`. Forces the `LazyLock` init for both runtimes and
    /// sanity-checks a couple of common pallets are present, so a
    /// broken artifact fails this test instead of a pallet-specific
    /// query far downstream.
    #[test]
    fn both_runtimes_load_and_resolve_common_pallets() {
        let allfeat = &*ALLFEAT_RUNTIME.decoded;
        assert!(allfeat.pallet_by_name("System").is_some());
        assert!(allfeat.pallet_by_name("Balances").is_some());

        let melodie = &*MELODIE_RUNTIME.decoded;
        assert!(melodie.pallet_by_name("System").is_some());
        assert!(melodie.pallet_by_name("Balances").is_some());
    }

    /// Every supported runtime metadata version (V14, V15, V16) is a
    /// valid decode target for subxt; the detector must report one of
    /// those three for each bundled blob so the frontend's tile doesn't
    /// render "V0" when the artifact is swapped. The exact number is
    /// deployment-specific — locking to the supported range rather than
    /// a literal keeps the test stable when the bundled blobs are
    /// regenerated against newer runtimes.
    #[test]
    fn metadata_versions_are_supported_variants() {
        for (id, version) in [
            ("allfeat", ALLFEAT_RUNTIME.version),
            ("melodie", MELODIE_RUNTIME.version),
        ] {
            assert!(
                (14..=16).contains(&version),
                "metadata version {version} for {id} outside the subxt-supported 14..=16 range",
            );
        }
    }

    /// `runtime_for` must dispatch each id to its own bundle — a
    /// regression here would silently route every Melodie decode through
    /// the Allfeat blob (and vice versa) and corrupt every pallet read.
    #[test]
    fn runtime_for_dispatches_per_network() {
        assert!(std::ptr::eq(runtime_for("allfeat"), &ALLFEAT_RUNTIME));
        assert!(std::ptr::eq(runtime_for("melodie"), &MELODIE_RUNTIME));
    }

    /// Decoding against an unknown pallet/variant is a no-op — the
    /// queries layer relies on this to keep its error surface
    /// uniform with "no fields decoded".
    #[cfg(not(feature = "mock"))]
    #[test]
    fn decode_unknown_pallet_returns_empty() {
        let fields = decode_event_fields("allfeat", "NotAPallet", "Whatever", &[], 42);
        assert!(fields.is_empty());
    }

    /// Truncated / garbage bytes must not panic the decoder. A bad
    /// blob on disk would otherwise take down every query that
    /// touches the affected event.
    #[cfg(not(feature = "mock"))]
    #[test]
    fn decode_malformed_bytes_returns_empty() {
        let fields = decode_event_fields("allfeat", "System", "ExtrinsicSuccess", &[0xff], 42);
        assert!(fields.is_empty());
    }

    /// The `/metadata/pallets` endpoint is only useful if the list is
    /// non-empty and matches pallets that actually emit extrinsics —
    /// `System` and `Balances` are ubiquitous on Substrate runtimes and
    /// both expose callable extrinsics, so a missing entry would flag a
    /// bad metadata blob. Checks both runtimes since the dropdown
    /// surfaces the per-network catalogue.
    #[test]
    fn callable_pallet_names_lists_system_and_balances_per_runtime() {
        for id in ["allfeat", "melodie"] {
            let names = callable_pallet_names(id);
            assert!(
                names.iter().any(|n| n == "System"),
                "[{id}] System missing; got {names:?}",
            );
            assert!(
                names.iter().any(|n| n == "Balances"),
                "[{id}] Balances missing; got {names:?}",
            );
        }
    }

    /// A known call (`Timestamp.set`) decodes into a single `now`
    /// parameter. `now` is `Compact<T::Moment>` — encode the payload by
    /// hand via `codec::Compact` and run it through the decoder so the
    /// field lookup, `scale_decode` walk, and value rendering are
    /// exercised end-to-end without leaning on the generated call type's
    /// internals.
    #[cfg(not(feature = "mock"))]
    #[test]
    fn decode_call_fields_round_trips_timestamp_set() {
        use subxt::ext::codec::{Compact, Encode};
        let now: u64 = 1_700_000_000_000;
        let bytes = Compact(now).encode();
        let fields = decode_call_fields("allfeat", "Timestamp", "set", &bytes, 42);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name.as_deref(), Some("now"));
        let parsed: u64 = fields[0]
            .value
            .parse()
            .unwrap_or_else(|_| panic!("`now` not a plain integer: {:?}", fields[0].value));
        assert_eq!(parsed, now);
    }

    /// An unknown call name on a real pallet — same shape contract as
    /// the unknown-pallet path. Keeps the fallback behaviour stable so
    /// the read path can always treat an empty vec as "fall back to
    /// Raw{hex}".
    #[cfg(not(feature = "mock"))]
    #[test]
    fn decode_call_fields_unknown_call_returns_empty() {
        let fields = decode_call_fields("allfeat", "Balances", "not_a_call", &[], 42);
        assert!(fields.is_empty());
    }

    /// Truncated bytes for a real call: must not panic and returns an
    /// empty vec — decoder is total, same contract as
    /// [`decode_event_fields`].
    #[cfg(not(feature = "mock"))]
    #[test]
    fn decode_call_fields_malformed_bytes_returns_empty() {
        let fields = decode_call_fields("allfeat", "Timestamp", "set", &[0xff], 42);
        assert!(fields.is_empty());
    }

    /// `decode_call_args` consumes subxt's `call_data_bytes()` shape
    /// (`[pallet_idx, call_idx, …field_bytes]`) — the two leading bytes
    /// must be stripped before the field walk. Regression test for the
    /// initial wiring that forwarded the prefix into the decoder and
    /// silently collapsed every DB-read extrinsic to `Raw`.
    #[cfg(not(feature = "mock"))]
    #[test]
    fn decode_call_args_strips_pallet_and_call_prefix() {
        use subxt::ext::codec::{Compact, Encode};
        let now: u64 = 1_700_000_000_000;
        // Arbitrary prefix bytes — the decoder must ignore them and
        // trust the `(pallet_name, call_name)` pair to resolve the
        // variant. Any two bytes work because we look up by name.
        let mut bytes = vec![0x02, 0x00];
        bytes.extend_from_slice(&Compact(now).encode());
        match decode_call_args("allfeat", "Timestamp", "set", &bytes, 42) {
            ExtrinsicArgs::Decoded { fields } => {
                assert_eq!(fields.len(), 1);
                assert_eq!(fields[0].name.as_deref(), Some("now"));
                assert_eq!(fields[0].value.parse::<u64>().unwrap(), now);
            }
            ExtrinsicArgs::Raw { hex } => {
                panic!("expected Decoded, got Raw({hex})");
            }
        }
    }

    /// A buffer shorter than 2 bytes can't carry a `(pallet, call)`
    /// prefix — `decode_call_args` must fall back to `Raw` rather than
    /// slicing into an empty window and emitting phantom `Decoded`.
    #[cfg(not(feature = "mock"))]
    #[test]
    fn decode_call_args_short_buffer_falls_back_to_raw() {
        match decode_call_args("allfeat", "Timestamp", "set", &[0xff], 42) {
            ExtrinsicArgs::Raw { hex } => assert_eq!(hex, "0xff"),
            other => panic!("expected Raw, got {other:?}"),
        }
    }

    /// Real-world `Balances.transfer_allow_death` payload captured from
    /// a live indexed block (the screenshot that flagged the initial
    /// "always Raw" bug). End-to-end check that `decode_call_args`
    /// produces the decoded fields with the `dest` SS58-rendered and
    /// `value` as a plain planck integer — the two forms the UI
    /// actually renders. Guards against regressions in both the prefix
    /// stripping and the `render_value` path.
    #[cfg(not(feature = "mock"))]
    #[test]
    fn decode_call_args_renders_balances_transfer_cleanly() {
        fn from_hex(s: &str) -> Vec<u8> {
            let s = s.strip_prefix("0x").unwrap_or(s);
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                .collect()
        }
        let bytes = from_hex(
            "0x05030056d264f360418e53f860338a37b8d42a28a10a347b1c96cbd6f62964ec92c21907bb39f9c601",
        );
        match decode_call_args("allfeat", "Balances", "transfer_allow_death", &bytes, 42) {
            ExtrinsicArgs::Decoded { fields } => {
                let dest = fields
                    .iter()
                    .find(|f| f.name.as_deref() == Some("dest"))
                    .expect("dest field");
                assert!(
                    dest.value.starts_with("5") && dest.value.len() >= 47,
                    "dest not SS58-rendered: {:?}",
                    dest.value
                );
                let value = fields
                    .iter()
                    .find(|f| f.name.as_deref() == Some("value"))
                    .expect("value field");
                assert_eq!(value.value.parse::<u128>().unwrap(), 7_633_189_307);
            }
            other => panic!("expected Decoded, got {other:?}"),
        }
    }
}
