//! Serde adapters that render integer types wider than 32 bits as JSON
//! strings.
//!
//! JS `Number` only holds 53 significant bits — a u64 block number or
//! u128 balance would silently truncate on the way through the Nuxt
//! frontend. These adapters keep one consistent rule across domain
//! types, API response wrappers, and indexer status: all ints ≥ 64 bits
//! serialise as JSON strings, deserialise via `FromStr`.
//!
//! Attach per-field:
//!
//! ```ignore
//! #[serde(with = "crate::serde_helpers::u128_string")]
//! #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
//! pub fee: u128,
//! ```
//!
//! Optionals get the `opt` twin so `None` round-trips as JSON `null`:
//!
//! ```ignore
//! #[serde(with = "crate::serde_helpers::u64_string_opt")]
//! #[cfg_attr(feature = "ts-bindings", ts(type = "string | null"))]
//! pub finalized_head: Option<u64>,
//! ```

pub mod u64_string {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(value)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        String::deserialize(d)?
            .parse::<u64>()
            .map_err(D::Error::custom)
    }
}

pub mod u128_string {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &u128, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(value)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u128, D::Error> {
        String::deserialize(d)?
            .parse::<u128>()
            .map_err(D::Error::custom)
    }
}

pub mod u64_string_opt {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Option<u64>, s: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(v) => s.collect_str(v),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
        let opt: Option<String> = Option::deserialize(d)?;
        match opt {
            Some(s) => s.parse::<u64>().map(Some).map_err(D::Error::custom),
            None => Ok(None),
        }
    }
}
