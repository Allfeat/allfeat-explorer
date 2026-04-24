//! Per-resource filter structs for the paginated list endpoints.
//!
//! Populated in Phase 3 of the pagination redesign (see
//! `docs/api-pagination-plan.md`). Each struct is deserialized by `axum`
//! from query parameters (`?finalized=true&min_extrinsics=1`), threaded
//! through [`crate::data::provider::ChainData`], and mapped to a `WHERE`
//! clause by the SQL layer. Filter fields are all `Option` so an absent
//! parameter is indistinguishable from the default — the common case.
//!
//! The structs aren't a generic DSL on purpose: column-level filters get
//! type-safe names, bindings stay obvious in TypeScript, and each query
//! can pick indexes tailored to its own shape.
//!
//! Fields marked **Phase 3 priority** were the ones the plan required to
//! replace the client-side filter scaffolding that used to live in
//! `pages/*/index.vue`. Secondary fields (e.g. `BlockFilters::author`) are
//! reserved in the plan but left out until there's UX need — adding them
//! now would ship dead code we can't exercise.

use serde::{Deserialize, Deserializer, Serialize};

use crate::domain::{Block, BlockEvent, CallResult, Extrinsic, Transfer};

/// Accept `success` / `failed` (the natural URL form) *and* the
/// capitalized `Success` / `Failed` that [`CallResult`] emits on the wire.
/// A strict default deserializer would reject `?status=success` because
/// `CallResult` serializes with variant-case — the custom path covers
/// both without touching the shared serialization.
fn deserialize_call_result_opt<'de, D>(d: D) -> Result<Option<CallResult>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw: Option<String> = Option::deserialize(d)?;
    match raw.as_deref() {
        None => Ok(None),
        Some("") => Ok(None),
        Some(s) if s.eq_ignore_ascii_case("success") => Ok(Some(CallResult::Success)),
        Some(s) if s.eq_ignore_ascii_case("failed") => Ok(Some(CallResult::Failed)),
        Some(other) => Err(serde::de::Error::custom(format!(
            "invalid status '{other}', expected 'success' or 'failed'"
        ))),
    }
}

#[cfg(feature = "ts-bindings")]
use ts_rs::TS;

/// Filters for `/blocks`. Phase 3 ships `finalized` and `min_extrinsics`;
/// `spec_version` / `author` stay reserved until the UI actually needs
/// them.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockFilters {
    /// `true` → only finalized blocks, `false` → only non-finalized. The
    /// indexed path stores nothing but finalized rows, so `Some(false)`
    /// short-circuits to an empty page there; the mock surface still has
    /// a "pending tip" window where the distinction is meaningful.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finalized: Option<bool>,
    /// Minimum number of extrinsics in the block. Cheap because the count
    /// is already denormalized on `blocks.extrinsic_count` — no join back
    /// to `extrinsics`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_extrinsics: Option<u32>,
}

/// Filters for `/extrinsics`. Phase 3 ships the four UX-critical fields
/// (`signed`, `status`, `pallet`, `call`); `signer` is reserved because a
/// proper address lookup belongs on the account detail page, not in the
/// generic feed.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtrinsicFilters {
    /// `true` → only signed extrinsics, `false` → only unsigned
    /// (inherents). Maps to `signer IS NOT NULL` / `signer IS NULL` on
    /// the SQL side. The default feed still hides the per-block
    /// timestamp inherent (`idx=0 + unsigned`) to avoid drowning the
    /// list; `signed == Some(false)` or `pallet == "Timestamp"` opens
    /// the hole back up so the caller can actually see it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed: Option<bool>,
    /// Exact pallet match (case-sensitive — metadata pallets are
    /// `snake_case` on substrate). Uses the
    /// `extrinsics_pallet_call_idx` composite index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pallet: Option<String>,
    /// Exact call name. Usually paired with `pallet` so the composite
    /// index can prune aggressively; alone it still falls back to a
    /// secondary index scan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<String>,
    /// Success / Failed. Maps to `success = true` / `success = false`.
    /// Accepts `success` / `failed` in query params too — [`CallResult`]'s
    /// default deserializer would only take `Success` / `Failed`, which
    /// looks wrong in a URL.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_call_result_opt"
    )]
    pub status: Option<CallResult>,
}

/// Filters for `/transfers`. Phase 3 ships `from` / `to`; `min_amount`
/// is reserved — the UI doesn't render an amount filter yet and a
/// u128-over-string comparison adds a SQL cast we'd rather avoid until
/// it's needed.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferFilters {
    /// Sender SS58. Invalid addresses short-circuit to `BadRequest`
    /// upstream so the user gets a 400 instead of a silent empty page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Recipient SS58. Same error contract as `from`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

/// Filters for `/events`. Uses the `events_pallet_variant_idx` composite
/// index — querying by `variant` alone still hits the index prefix.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventFilters {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pallet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

/// Filters for `/ats`. Placeholder — no fields planned yet.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtsFilters {}

/// Filters for `/ats/feed`. Placeholder.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtsFeedFilters {}

/// Filters for `/accounts/{address}/ats`. Placeholder.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountAtsFilters {}

// ── In-memory predicates ───────────────────────────────────────────────
//
// The mock surface and the RPC fallback filter rows in-memory (no SQL),
// and the indexed provider's pending-slice path does too. Sharing the
// match helpers keeps those three paths aligned — a DB WHERE clause and
// an in-memory predicate are two different ways to answer the same
// question, so they must agree on the semantics of each field.

/// Predicate matching [`BlockFilters`] against a single [`Block`].
/// Returns `true` when the block passes every populated filter — absent
/// fields never exclude rows.
pub fn block_matches_filters(block: &Block, filters: &BlockFilters) -> bool {
    if let Some(finalized) = filters.finalized {
        if block.finalized != finalized {
            return false;
        }
    }
    if let Some(min) = filters.min_extrinsics {
        if block.extrinsic_count < min {
            return false;
        }
    }
    true
}

/// Predicate matching [`ExtrinsicFilters`] against a single extrinsic.
///
/// `pallet` / `call` are exact matches — matches the SQL path which
/// compares the text columns with `=`, not `ILIKE`. A UX "search"
/// helper that allows prefixes is a separate concern.
pub fn extrinsic_matches_filters(ext: &Extrinsic, filters: &ExtrinsicFilters) -> bool {
    if let Some(signed) = filters.signed {
        if ext.signed != signed {
            return false;
        }
    }
    if let Some(pallet) = filters.pallet.as_deref() {
        if ext.module != pallet {
            return false;
        }
    }
    if let Some(call) = filters.call.as_deref() {
        if ext.call != call {
            return false;
        }
    }
    if let Some(status) = filters.status {
        if ext.result != status {
            return false;
        }
    }
    true
}

/// Predicate matching [`TransferFilters`] against a single transfer.
/// `from` / `to` are compared as SS58 strings — the domain type already
/// stores them that way, so no re-encoding is needed here.
pub fn transfer_matches_filters(transfer: &Transfer, filters: &TransferFilters) -> bool {
    if let Some(from) = filters.from.as_deref() {
        if transfer.from != from {
            return false;
        }
    }
    if let Some(to) = filters.to.as_deref() {
        if transfer.to != to {
            return false;
        }
    }
    true
}

/// Predicate matching [`EventFilters`] against a single event row.
pub fn event_matches_filters(event: &BlockEvent, filters: &EventFilters) -> bool {
    if let Some(pallet) = filters.pallet.as_deref() {
        if event.module != pallet {
            return false;
        }
    }
    if let Some(variant) = filters.variant.as_deref() {
        if event.name != variant {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        Block, BlockEvent, CallResult, EventPhase, Extrinsic, ExtrinsicArgs, Transfer,
    };

    // Small builders keep each assertion focused on the one field under
    // test — a full domain struct per assertion would drown the intent in
    // boilerplate.
    fn block(number: u64, finalized: bool, extrinsic_count: u32) -> Block {
        Block {
            number,
            hash: String::new(),
            parent_hash: String::new(),
            state_root: String::new(),
            extrinsics_root: String::new(),
            timestamp_ms: 0,
            finalized,
            extrinsic_count,
            event_count: 0,
            author: String::new(),
            author_name: String::new(),
            ref_time: 0,
            ref_time_pct: 0,
            proof_size: 0,
            spec_version: 0,
            size_bytes: 0,
        }
    }

    fn extrinsic(module: &str, call: &str, signed: bool, result: CallResult) -> Extrinsic {
        Extrinsic {
            id: "1-0".into(),
            block_number: 1,
            index: 0,
            hash: String::new(),
            module: module.into(),
            call: call.into(),
            signed,
            signer: signed.then(|| "signer-addr".into()),
            args: ExtrinsicArgs::Raw { hex: String::new() },
            result,
            nonce: None,
            tip: 0,
            fee: 0,
            timestamp_ms: 0,
            events: Vec::new(),
        }
    }

    fn event(module: &str, name: &str) -> BlockEvent {
        BlockEvent {
            block_number: 1,
            index: 0,
            phase: EventPhase::Initialization,
            module: module.into(),
            name: name.into(),
            fields: Vec::new(),
            timestamp_ms: 0,
        }
    }

    fn transfer(from: &str, to: &str) -> Transfer {
        Transfer {
            extrinsic: extrinsic("balances", "transfer", true, CallResult::Success),
            from: from.into(),
            to: to.into(),
            amount: 0,
        }
    }

    #[test]
    fn default_block_filters_let_everything_through() {
        let filters = BlockFilters::default();
        assert!(block_matches_filters(&block(10, true, 0), &filters));
        assert!(block_matches_filters(&block(10, false, 0), &filters));
    }

    #[test]
    fn block_filters_honor_finalized_and_min_extrinsics() {
        let finalized_only = BlockFilters {
            finalized: Some(true),
            min_extrinsics: None,
        };
        assert!(block_matches_filters(&block(1, true, 0), &finalized_only));
        assert!(!block_matches_filters(&block(1, false, 0), &finalized_only));

        let busy_only = BlockFilters {
            finalized: None,
            min_extrinsics: Some(5),
        };
        assert!(block_matches_filters(&block(1, true, 5), &busy_only));
        assert!(!block_matches_filters(&block(1, true, 4), &busy_only));
    }

    #[test]
    fn extrinsic_filters_combine_conjunctively() {
        let filters = ExtrinsicFilters {
            signed: Some(true),
            pallet: Some("balances".into()),
            call: None,
            status: Some(CallResult::Success),
        };
        assert!(extrinsic_matches_filters(
            &extrinsic("balances", "transfer", true, CallResult::Success),
            &filters,
        ));
        // Fails any field → drops the row.
        assert!(!extrinsic_matches_filters(
            &extrinsic("balances", "transfer", false, CallResult::Success),
            &filters,
        ));
        assert!(!extrinsic_matches_filters(
            &extrinsic("system", "remark", true, CallResult::Success),
            &filters,
        ));
        assert!(!extrinsic_matches_filters(
            &extrinsic("balances", "transfer", true, CallResult::Failed),
            &filters,
        ));
    }

    #[test]
    fn transfer_filters_match_from_and_to_strings() {
        let filters = TransferFilters {
            from: Some("alice".into()),
            to: Some("bob".into()),
        };
        assert!(transfer_matches_filters(
            &transfer("alice", "bob"),
            &filters
        ));
        assert!(!transfer_matches_filters(
            &transfer("alice", "charlie"),
            &filters
        ));
        assert!(!transfer_matches_filters(&transfer("eve", "bob"), &filters));
    }

    #[test]
    fn event_filters_match_module_and_name() {
        let filters = EventFilters {
            pallet: Some("balances".into()),
            variant: Some("Transfer".into()),
        };
        assert!(event_matches_filters(
            &event("balances", "Transfer"),
            &filters
        ));
        assert!(!event_matches_filters(
            &event("balances", "Deposit"),
            &filters
        ));
        assert!(!event_matches_filters(
            &event("system", "Transfer"),
            &filters
        ));
    }

    #[test]
    fn status_deserializes_case_insensitively() {
        // Mirrors what `?status=success` / `?status=Success` lands as when
        // axum decodes the query string.
        let cases: &[(&str, Option<CallResult>)] = &[
            ("success", Some(CallResult::Success)),
            ("Success", Some(CallResult::Success)),
            ("failed", Some(CallResult::Failed)),
            ("FAILED", Some(CallResult::Failed)),
        ];
        for (raw, expected) in cases {
            let query = format!("status={raw}");
            let filters: ExtrinsicFilters = serde_urlencoded::from_str(&query)
                .unwrap_or_else(|e| panic!("decode '{query}' failed: {e}"));
            assert_eq!(
                filters.status, *expected,
                "status={raw} decoded to {:?}",
                filters.status
            );
        }

        // Unknown values must surface as a decode error so the handler
        // sends a 400 instead of silently falling back to "any status".
        let bad: Result<ExtrinsicFilters, _> = serde_urlencoded::from_str("status=exploded");
        assert!(bad.is_err(), "invalid status should fail deserialization");
    }
}
