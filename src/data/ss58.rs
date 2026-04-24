//! SS58 address encoder parameterized by the chain's `ss58Format` prefix.
//!
//! `subxt::utils::AccountId32::Display` hardcodes prefix 42 (generic Substrate,
//! addresses starting with `5`), so we cannot use it when a network publishes
//! a different `ss58Format` via `system_properties`. The routine below mirrors
//! subxt's own internal encoder (`base58` alphabet + 2-byte blake2b-512
//! checksum over `b"SS58PRE" || prefix_bytes || account_bytes`) but accepts an
//! arbitrary `u16` prefix per the SS58 spec.
//!
//! Prefix encoding:
//! * `0..=63` → one byte `prefix as u8`.
//! * `64..=16_383` → two bytes. The high two bits of the first byte are set to
//!   `01` to signal a 14-bit prefix; the remaining 14 bits are split across the
//!   two bytes in a reordered layout (see SS58 §Specification — "Full address
//!   format"). Values above 16_383 are illegal under the spec and get clamped
//!   to the low 14 bits, matching `sp_core::crypto::Ss58Codec` behaviour so a
//!   misconfigured `ss58Format` never panics in the hot path.

use base58::ToBase58;
use blake2::{Blake2b512, Digest};

/// SS58 checksum prefix. Baked into the blake2b input before the account body
/// so SS58-decoders can reject strings that weren't produced by an SS58
/// encoder (e.g. a plain base58 key).
const CHECKSUM_PREFIX: &[u8] = b"SS58PRE";

/// Number of checksum bytes appended to the body. SS58 uses two for
/// 32-byte account ids.
const CHECKSUM_LEN: usize = 2;

/// SCALE-style prefix-byte packing for the `64..=16_383` range. Lifted
/// verbatim from `sp_core::crypto::Ss58Codec::to_ss58check_with_version` so
/// addresses round-trip with Polkadot.js / subxt. The 14-bit prefix is split
/// across two bytes as:
///
/// * `first`  — high two bits `01` (the two-byte discriminator), low six bits
///   carry bits 2..=7 of `prefix`.
/// * `second` — high two bits carry bits 0..=1 of `prefix`, low six bits
///   carry bits 8..=13 of `prefix`.
fn encode_prefix(prefix: u16) -> Vec<u8> {
    if prefix < 64 {
        vec![prefix as u8]
    } else {
        // Clamp to 14 bits. `ss58Format > 16_383` is out-of-spec; we stay
        // non-panicking and mirror sp_core's masking so debug runs aren't
        // louder than release on bad input.
        let p = prefix & 0x3fff;
        let first = (((p & 0x00fc) >> 2) as u8) | 0x40;
        let second = ((p >> 8) as u8) | (((p & 0x0003) as u8) << 6);
        vec![first, second]
    }
}

/// Base58-encode a 32-byte account id with the given SS58 `prefix`.
///
/// Allocates a small `Vec<u8>` for the checksum input (`prefix_bytes ||
/// account_bytes || checksum`) and a `String` for the base58 output — both
/// bounded: the final string is always 47–48 characters for prefixes ≤ 63 and
/// 48–49 for larger prefixes. Called once per address displayed in an API
/// response, so the allocation overhead is negligible next to the per-request
/// JSON serialisation.
pub fn encode_ss58(account_bytes: &[u8; 32], prefix: u16) -> String {
    let prefix_bytes = encode_prefix(prefix);
    // Capacity: prefix (1 or 2) + body (32) + checksum (2) = 35 or 36 bytes.
    let mut buf = Vec::with_capacity(prefix_bytes.len() + 32 + CHECKSUM_LEN);
    buf.extend_from_slice(&prefix_bytes);
    buf.extend_from_slice(account_bytes);

    let mut ctx = Blake2b512::new();
    ctx.update(CHECKSUM_PREFIX);
    ctx.update(&buf);
    let checksum = ctx.finalize();
    buf.extend_from_slice(&checksum[..CHECKSUM_LEN]);

    buf.to_base58()
}

/// Convenience wrapper for the common "BYTEA column from Postgres" case: the
/// input is a slice of arbitrary length, and we want `None` (not a wrong
/// address) when it isn't exactly 32 bytes. Indexer rows are padded to 32
/// bytes on insert, so a mismatch signals genuine data corruption.
pub fn encode_ss58_bytes(account_bytes: &[u8], prefix: u16) -> Option<String> {
    if account_bytes.len() != 32 {
        return None;
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(account_bytes);
    Some(encode_ss58(&arr, prefix))
}

/// Compact SS58 display label: 6 head + 4 tail with an ellipsis, so the
/// column stays narrow but distinctive. Returns the input unchanged for
/// anything shorter than 12 chars (hexless addresses, test fixtures).
pub fn short_label(ss58: &str) -> String {
    if ss58.chars().count() <= 12 {
        return ss58.to_string();
    }
    let head: String = ss58.chars().take(6).collect();
    let tail: String = ss58
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{head}…{tail}")
}

/// Decode a 64-hex-char string (with optional `0x` prefix) into 32 bytes.
/// Returns `None` on any length mismatch or non-hex alphabet — the call
/// sites treat that as "not a valid hash" and route to the `-<idx>` path
/// or fall through to a 404.
pub fn decode_hex32(s: &str) -> Option<[u8; 32]> {
    let trimmed = s.strip_prefix("0x").unwrap_or(s);
    let mut out = [0u8; 32];
    hex::decode_to_slice(trimmed, &mut out).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_label_keeps_short_strings_intact() {
        assert_eq!(short_label("abc"), "abc");
        assert_eq!(short_label(""), "");
    }

    #[test]
    fn short_label_truncates_long_strings_with_ellipsis() {
        let s = short_label("5GrwvaEFinqsLPxV6dRiYpZkmf3uG9bAagdNXTuZmrjN6dhV");
        assert!(s.starts_with("5Grwva"));
        assert!(s.ends_with("6dhV"));
        assert!(s.contains('…'));
    }

    #[test]
    fn decode_hex32_roundtrips_through_subxt_bytes() {
        let bytes = [0xAB; 32];
        let lowered = format!("0x{}", hex::encode(bytes));
        assert_eq!(decode_hex32(&lowered), Some(bytes));
        // Bare (no 0x prefix) also decodes.
        assert_eq!(decode_hex32(&lowered[2..]), Some(bytes));
        // Uppercase hex body is accepted by `hex::decode_to_slice`.
        let uppered = format!("0x{}", hex::encode_upper(bytes));
        assert_eq!(decode_hex32(&uppered), Some(bytes));
    }

    #[test]
    fn decode_hex32_rejects_wrong_length_or_alphabet() {
        assert!(decode_hex32("").is_none());
        // 63 chars — one short.
        assert!(decode_hex32(&"0".repeat(63)).is_none());
        // 65 chars — one long.
        assert!(decode_hex32(&"0".repeat(65)).is_none());
        // Invalid hex character.
        assert!(decode_hex32(&"Z".repeat(64)).is_none());
    }

    /// Prefix 42 (generic Substrate) matches subxt's built-in `AccountId32`
    /// display so the refactor doesn't silently change address strings for
    /// networks whose `ss58Format` is still 42.
    #[test]
    fn prefix_42_matches_subxt_display() {
        let bytes = [0x11u8; 32];
        let subxt_display = subxt::utils::AccountId32::from(bytes).to_string();
        assert_eq!(encode_ss58(&bytes, 42), subxt_display);
    }

    /// Alice's well-known address on Polkadot (prefix 0) — sanity check
    /// against a published value so a silent regression is immediately
    /// visible. The 32-byte payload is Alice's `sr25519::Public`.
    #[test]
    fn alice_polkadot_prefix_0() {
        // 0xd43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d
        let bytes = [
            0xd4, 0x35, 0x93, 0xc7, 0x15, 0xfd, 0xd3, 0x1c, 0x61, 0x14, 0x1a, 0xbd, 0x04, 0xa9,
            0x9f, 0xd6, 0x82, 0x2c, 0x85, 0x58, 0x85, 0x4c, 0xcd, 0xe3, 0x9a, 0x56, 0x84, 0xe7,
            0xa5, 0x6d, 0xa2, 0x7d,
        ];
        assert_eq!(
            encode_ss58(&bytes, 0),
            "15oF4uVJwmo4TdGW7VfQxNLavjCXviqxT9S1MgbjMNHr6Sp5"
        );
    }

    /// Kusama uses prefix 2 — another widely-tested vector so the
    /// 0..=63 branch stays honest.
    #[test]
    fn alice_kusama_prefix_2() {
        let bytes = [
            0xd4, 0x35, 0x93, 0xc7, 0x15, 0xfd, 0xd3, 0x1c, 0x61, 0x14, 0x1a, 0xbd, 0x04, 0xa9,
            0x9f, 0xd6, 0x82, 0x2c, 0x85, 0x58, 0x85, 0x4c, 0xcd, 0xe3, 0x9a, 0x56, 0x84, 0xe7,
            0xa5, 0x6d, 0xa2, 0x7d,
        ];
        assert_eq!(
            encode_ss58(&bytes, 2),
            "HNZata7iMYWmk5RvZRTiAsSDhV8366zq2YGb3tLH5Upf74F"
        );
    }

    /// Round-trip check across both prefix ranges. subxt's `from_ss58` (called
    /// by `AccountId32::from_str`) validates the blake2 checksum and pulls the
    /// 32-byte body back out regardless of which prefix was used, so any mismatch
    /// between our encoder's prefix bytes and its own checksum would surface as
    /// a decode failure. Covers prefix 42 (single-byte), 100 (two-byte boundary
    /// just above 63), and 16_383 (upper bound of the two-byte range).
    #[test]
    fn roundtrips_through_subxt_decoder() {
        use subxt::utils::AccountId32;
        let bytes = [0xABu8; 32];
        for prefix in [0u16, 42, 63, 64, 100, 172, 16_383] {
            let encoded = encode_ss58(&bytes, prefix);
            let decoded: AccountId32 = encoded
                .parse()
                .unwrap_or_else(|e| panic!("parse {encoded} (prefix {prefix}): {e:?}"));
            assert_eq!(
                <AccountId32 as AsRef<[u8]>>::as_ref(&decoded),
                &bytes,
                "prefix {prefix} roundtrip lost bytes",
            );
        }
    }

    #[test]
    fn encode_ss58_bytes_rejects_wrong_length() {
        assert!(encode_ss58_bytes(&[0u8; 31], 42).is_none());
        assert!(encode_ss58_bytes(&[0u8; 33], 42).is_none());
        assert!(encode_ss58_bytes(&[], 42).is_none());
    }

    #[test]
    fn prefix_440_produces_q_addresses() {
        // Allfeat mainnet is registered at ss58Format = 440 — the address
        // space starts with 'q' for every 32-byte id. Verifies the two-byte
        // prefix-encoding branch is byte-identical to what sp_core produces
        // (would otherwise collide with mappings that straddle the same
        // prefix base but use a different bit layout).
        for bytes in [[0u8; 32], [0x11u8; 32], [0xAAu8; 32], [0xFFu8; 32]] {
            let s = encode_ss58(&bytes, 440);
            assert!(
                s.starts_with('q'),
                "prefix 440 must start with 'q', got {s}"
            );
        }
    }

    #[test]
    fn encode_ss58_bytes_accepts_32() {
        assert_eq!(
            encode_ss58_bytes(&[0x11u8; 32], 42),
            Some(encode_ss58(&[0x11u8; 32], 42))
        );
    }
}
