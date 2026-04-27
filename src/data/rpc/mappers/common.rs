//! Helpers shared across every mapper submodule: hash/hex formatting,
//! SS58 coercion, and the block-level event index.

use std::collections::HashMap;

use subxt::client::OnlineClientAtBlock;
use subxt::events::{Event, Events, Phase};
use subxt::ext::codec::Decode;
use subxt::utils::{AccountId32, MultiAddress, H256};
use subxt::SubstrateConfig;

use crate::data::error::{DataError, DataResult};
use crate::data::rpc::client::with_timeout;
use crate::data::ss58::encode_ss58;
use crate::domain::{BlockEvent, EventPhase};

/// `ApplyExtrinsic`-scoped events grouped by their extrinsic index. Built once
/// per block via [`index_events_by_phase`] so mappers that walk the extrinsic
/// list look up their matching events in O(1) instead of re-filtering the full
/// event set on every extrinsic.
///
/// Carries the lifetime of the [`Events`] buffer it was indexed from: subxt's
/// `Event<'_, T>` borrows the metadata and name slices rather than owning
/// them, so callers must keep the parent `Events` alive for the duration of
/// the lookup.
pub type EventsByPhase<'a> = HashMap<u32, Vec<Event<'a, SubstrateConfig>>>;

/// Map subxt's `Phase` onto the domain [`EventPhase`]. Re-used by
/// [`map_block_events`] and the `/events` feed so the two never drift on
/// the Initialization / Finalization handling.
pub fn phase_to_domain(phase: Phase) -> EventPhase {
    match phase {
        Phase::Initialization => EventPhase::Initialization,
        Phase::ApplyExtrinsic(i) => EventPhase::ApplyExtrinsic { index: i },
        Phase::Finalization => EventPhase::Finalization,
    }
}

/// `0x` + 64 lowercase hex chars for the 32-byte hashes the explorer renders.
pub fn hash_string(h: &H256) -> String {
    hex_bytes(h.as_bytes())
}

/// `0x` + lowercase hex. Used for `ExtrinsicArgs::Raw` payloads and anywhere
/// an opaque SCALE blob needs to surface in the UI.
///
/// Uses a static nibble lookup instead of `write!("{:02x}")` — the `write!`
/// variant formats through `core::fmt` and ends up ~2× slower on the mapper
/// hot path (called from ~10 places per block decode).
pub fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(2 + bytes.len() * 2);
    out.extend_from_slice(b"0x");
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize]);
        out.push(HEX[(b & 0x0f) as usize]);
    }
    // All bytes pushed above are ASCII, so the UTF-8 invariant holds.
    String::from_utf8(out).expect("hex bytes are always valid ASCII/UTF-8")
}

/// Decode the signed-origin address bytes into an SS58-encoded string using
/// the network's `ss58Format` prefix (read from `system_properties`, threaded
/// down from the provider).
///
/// Substrate's `MultiAddress` supports several variants; only `Id(AccountId32)`
/// actually appears for normally-signed extrinsics. The other variants (raw,
/// 20-byte, index) are either chain-specific or rare — surface them as `None`
/// so the UI shows "—" rather than a misleading string.
pub fn decode_signer_ss58(bytes: &[u8], prefix: u16) -> Option<String> {
    let addr = MultiAddress::<AccountId32, u32>::decode(&mut &bytes[..]).ok()?;
    match addr {
        MultiAddress::Id(id) => Some(encode_ss58(&id.0, prefix)),
        _ => None,
    }
}

/// Encode an `AccountId32` as an SS58 string under the network's own prefix.
/// Subxt's built-in `Display` always uses prefix 42, so every address-producing
/// mapper goes through this helper instead.
pub fn account_ss58(id: &AccountId32, prefix: u16) -> String {
    encode_ss58(&id.0, prefix)
}

/// Fetch the block's full event set. Always one round-trip to the node; used
/// as the input to [`index_events_by_phase`] so subsequent mappers can avoid
/// re-fetching.
pub async fn fetch_block_events(
    at: &OnlineClientAtBlock<SubstrateConfig>,
) -> DataResult<Events<SubstrateConfig>> {
    with_timeout("fetch_block_events", async {
        at.events()
            .fetch()
            .await
            .map_err(|e| DataError::Rpc(format!("fetch events: {e}")))
    })
    .await
}

/// Decode every event once and group the `ApplyExtrinsic`-scoped ones by
/// their extrinsic index. Events emitted by block init/finalization are
/// dropped — the extrinsic mapper (which is the only consumer of this map)
/// only attaches `ApplyExtrinsic`-phase events to extrinsics. The full
/// event list with phase info is exposed via [`map_block_events`].
///
/// Returns an `EventsByPhase` safe to share across `map_extrinsics`,
/// `map_transfers` and `fetch_version_extras`: each lookup is O(1) instead
/// of the previous O(E) re-scan per caller. The returned map borrows from
/// `events`, so the caller must keep that handle alive.
pub fn index_events_by_phase<'a>(
    events: &'a Events<SubstrateConfig>,
) -> DataResult<EventsByPhase<'a>> {
    let mut by_phase: EventsByPhase<'a> = HashMap::new();
    for evt in events.iter() {
        let evt = evt.map_err(|e| DataError::Decode(format!("decode event: {e}")))?;
        if let Phase::ApplyExtrinsic(idx) = evt.phase() {
            by_phase.entry(idx).or_default().push(evt);
        }
    }
    Ok(by_phase)
}

/// Decode every event in the block and surface it as a [`BlockEvent`],
/// preserving the phase. Unlike [`index_events_by_phase`] this keeps the
/// `Initialization` and `Finalization` events — Substrate emits those for
/// session-change effects (`session.NewSession`, `grandpa.NewAuthorities`,
/// etc.) and they'd otherwise vanish from the block-detail Events tab.
///
/// Running order matches the on-chain event stream, so the `index` field is
/// just the position in the iterator — this lines up with Substrate's
/// `<block>-<index>` convention for event ids.
pub fn map_block_events(
    block_number: u64,
    timestamp_ms: i64,
    events: &Events<SubstrateConfig>,
    network_id: &str,
    ss58_prefix: u16,
) -> DataResult<Vec<BlockEvent>> {
    let mut out = Vec::with_capacity(events.len() as usize);
    for (idx, evt) in events.iter().enumerate() {
        let evt = evt.map_err(|e| DataError::Decode(format!("decode event: {e}")))?;
        let phase = phase_to_domain(evt.phase());
        let module = evt.pallet_name().to_string();
        let name = evt.event_name().to_string();
        let fields = crate::data::metadata::decode_event_fields(
            network_id,
            &module,
            &name,
            evt.field_bytes(),
            ss58_prefix,
        );
        out.push(BlockEvent {
            block_number,
            index: idx as u32,
            phase,
            module,
            name,
            fields,
            timestamp_ms,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use subxt::ext::codec::Encode;

    #[test]
    fn hash_string_renders_lowercase_zero_x() {
        let s = hash_string(&H256::from([0xab; 32]));
        assert!(s.starts_with("0x"));
        assert_eq!(s.len(), 2 + 64);
        assert!(s[2..]
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(s, format!("0x{}", "ab".repeat(32)));
    }

    #[test]
    fn hex_bytes_renders_lowercase_zero_x_prefix() {
        assert_eq!(hex_bytes(&[]), "0x");
        assert_eq!(hex_bytes(&[0xab, 0xcd, 0xef]), "0xabcdef");
        assert_eq!(hex_bytes(&[0x00, 0x01]), "0x0001");
    }

    #[test]
    fn decode_signer_ss58_roundtrips_id_variant() {
        // Build a MultiAddress::Id(AccountId32) and SCALE-encode it the same
        // way the node does, then feed the bytes back through our decoder
        // with the default Substrate prefix (42) so the expected string can
        // be derived from subxt's built-in `Display`.
        let expected_id = AccountId32::from([7u8; 32]);
        let addr: MultiAddress<AccountId32, u32> = MultiAddress::Id(expected_id);
        let bytes = addr.encode();
        let ss58 = decode_signer_ss58(&bytes, 42).expect("Id variant should decode");
        assert_eq!(ss58, expected_id.to_string());
    }

    #[test]
    fn decode_signer_ss58_uses_network_prefix() {
        // Prefix 0 (Polkadot) and prefix 42 (generic) produce different
        // strings for the same account bytes — a round-trip proves the
        // prefix argument reaches the encoder.
        let id = AccountId32::from([7u8; 32]);
        let bytes = MultiAddress::<AccountId32, u32>::Id(id).encode();
        let dot = decode_signer_ss58(&bytes, 0).expect("decode");
        let generic = decode_signer_ss58(&bytes, 42).expect("decode");
        assert_ne!(
            dot, generic,
            "different prefixes must produce different strings"
        );
        assert!(
            dot.starts_with('1'),
            "Polkadot addresses start with '1': got {dot}"
        );
        assert!(
            generic.starts_with('5'),
            "generic addresses start with '5': got {generic}"
        );
    }

    #[test]
    fn decode_signer_ss58_rejects_non_id_variants() {
        // `Raw` is a legal MultiAddress variant but we surface it as None —
        // rendering it as SS58 would lie about what the on-chain address is.
        let addr: MultiAddress<AccountId32, u32> = MultiAddress::Raw(vec![1, 2, 3]);
        assert!(decode_signer_ss58(&addr.encode(), 42).is_none());
    }

    #[test]
    fn decode_signer_ss58_rejects_malformed_bytes() {
        assert!(decode_signer_ss58(&[], 42).is_none());
        assert!(decode_signer_ss58(&[0xff], 42).is_none());
    }
}
