//! Project a pinned `OnlineClientAtBlock` into `ExtrinsicRow`s ready
//! for the `extrinsics` table. Implemented in Phase 3 of
//! `docs/indexing-plan.md`.
//!
//! Two things distinguish this projection from [`crate::data::rpc::mappers::extrinsics`]:
//!
//! * **Byte shape.** We persist the raw 32-byte hash, the 32-byte
//!   signer, the SCALE-encoded call arguments — no SS58 strings, no
//!   hex. The sink binds these straight into `BYTEA` columns; a
//!   hash-lookup query downstream ends up as one memcmp against the
//!   index rather than a hex normalise + compare. The domain-level
//!   rendering is deferred to [`crate::data::indexed::queries`] on the
//!   read path.
//! * **Error unpacking.** The `extrinsics.error_module` /
//!   `extrinsics.error_name` columns are resolved via metadata on the
//!   way in so the UI doesn't have to hold the runtime types at query
//!   time. A `DispatchError::Module { index, error }` looks up the
//!   pallet name + error variant name through `at.metadata()`; any
//!   other dispatch-error variant (e.g. `BadOrigin`, `Token(...)`)
//!   surfaces as a non-Module failure with both columns left `NULL`
//!   and `success = false`.
//!
//! Keeping the transform pure (no DB, no network beyond what the pin
//! already provides) means the live and backfill workers share a single
//! implementation, and a fixture-driven unit test can exercise every
//! decoding branch without standing up Postgres.

use subxt::client::OnlineClientAtBlock;
use subxt::SubstrateConfig;

use crate::data::error::{DataError, DataResult};
use crate::data::rpc::client::with_timeout;
use crate::data::rpc::mappers::{fetch_block_events, index_events_by_phase};
use crate::data::rpc::runtime::allfeat;

/// Row shape for `extrinsics`. Field order mirrors the `INSERT`
/// statement in [`crate::indexer::sink::insert_extrinsics`]; adding a
/// column means touching both places deliberately rather than silently
/// forgetting one side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtrinsicRow {
    pub block_num: u64,
    pub idx: u32,
    pub hash: [u8; 32],
    pub pallet: String,
    pub call: String,
    /// `None` for inherents / unsigned extrinsics. `Some([u8; 32])`
    /// holds the raw `AccountId32` for the `Id` variant of
    /// `MultiAddress`. Non-Id variants (Raw, Index, ...) surface as
    /// `None` rather than a misleading byte pattern — the BYTEA column
    /// is nullable for exactly this reason.
    pub signer: Option<[u8; 32]>,
    /// Planck-denominated tip; `None` when the extrinsic carries no
    /// `ChargeTransactionPayment` extension (i.e. unsigned).
    pub tip: Option<u128>,
    /// Fee actually paid, sourced from
    /// `TransactionPayment.TransactionFeePaid`. Zero for inherents and
    /// any extrinsic that emits no fee event.
    pub fee: u128,
    /// Nonce from the signed extensions. `None` for unsigned.
    pub nonce: Option<u64>,
    pub success: bool,
    /// Set from `DispatchError::Module`. Non-Module failures (BadOrigin,
    /// Token, etc.) leave `error_module` / `error_name` both `None`
    /// even though `success = false` — the UI renders "failed" either
    /// way, and the few advanced callers that care can still tell the
    /// two apart via the NULL-vs-populated columns.
    pub error_module: Option<String>,
    pub error_name: Option<String>,
    /// Raw call-data bytes (SCALE-encoded call, NOT the whole
    /// extrinsic). Readers decode this on demand against the spec
    /// version's metadata — see [`crate::data::indexed::queries::extrinsics`].
    pub args_scale: Vec<u8>,
}

/// Project every extrinsic in the pinned block into an `ExtrinsicRow`.
///
/// `block_num` is passed in rather than inferred from `at.block_number()`
/// so the caller's logical cursor is the single source of truth — no way
/// for a buggy pin to stamp the wrong primary key.
pub async fn map(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    block_num: u64,
) -> DataResult<Vec<ExtrinsicRow>> {
    let extrinsics = with_timeout("map_extrinsics_fetch", async {
        at.extrinsics()
            .fetch()
            .await
            .map_err(|e| DataError::Rpc(format!("fetch extrinsics: {e}")))
    })
    .await?;

    // One block-level event fetch, then an O(1) index by phase. The
    // `DispatchError` decode on failure needs access to the metadata
    // we pull once up front via `at.metadata()`.
    let events = fetch_block_events(at).await?;
    let events_by_phase = index_events_by_phase(&events)?;
    let metadata = at.metadata();

    let mut out = Vec::with_capacity(extrinsics.len());
    for ext in extrinsics.iter() {
        let ext = ext.map_err(|e| DataError::Decode(format!("decode extrinsic: {e}")))?;
        let idx = ext.index() as u32;
        let pallet = ext.pallet_name().to_string();
        let call = ext.call_name().to_string();
        let hash = ext.hash().0;
        let signer = decode_signer_bytes(ext.address_bytes());
        let (nonce, tip) = ext
            .transaction_extensions()
            .map(|exts| (exts.nonce(), exts.tip()))
            .unwrap_or((None, None));

        let (success, error_module, error_name, fee) = match events_by_phase.get(&idx) {
            Some(evts) => resolve_outcome(evts, &metadata),
            None => (true, None, None, 0),
        };

        out.push(ExtrinsicRow {
            block_num,
            idx,
            hash,
            pallet,
            call,
            signer,
            tip,
            fee,
            nonce,
            success,
            error_module,
            error_name,
            args_scale: ext.call_data_bytes().to_vec(),
        });
    }
    Ok(out)
}

/// Walk the ApplyExtrinsic events for one extrinsic and extract:
///
/// * `success` — `false` iff we see an explicit `System.ExtrinsicFailed`,
///   matching subxt's convention. Inherents that emit no
///   `System.ExtrinsicSuccess` still count as successful (timestamp,
///   etc.); `ExtrinsicFailed` is the only on-chain signal of a revert.
/// * `error_module` / `error_name` — populated only when the
///   `DispatchError` is `Module(ModuleError { index, error })` and the
///   metadata resolves both halves. Any other variant leaves both
///   `None` while still flipping `success` to false.
/// * `fee` — pulled from `TransactionPayment.TransactionFeePaid` if
///   present. Inherents pay no fee, so the `0` default is correct for
///   them.
fn resolve_outcome(
    evts: &[subxt::events::Event<'_, SubstrateConfig>],
    metadata: &subxt::Metadata,
) -> (bool, Option<String>, Option<String>, u128) {
    let mut success = true;
    let mut error_module = None;
    let mut error_name = None;
    let mut fee = 0u128;

    for evt in evts {
        match (evt.pallet_name(), evt.event_name()) {
            ("System", "ExtrinsicFailed") => {
                success = false;
                if let Ok(failed) = evt
                    .decode_fields_unchecked_as::<allfeat::system::events::ExtrinsicFailed>()
                {
                    if let (Some(module), Some(name)) = resolve_module_error(&failed.dispatch_error, metadata) {
                        error_module = Some(module);
                        error_name = Some(name);
                    }
                }
            }
            ("TransactionPayment", "TransactionFeePaid") => {
                if let Ok(paid) = evt
                    .decode_fields_unchecked_as::<allfeat::transaction_payment::events::TransactionFeePaid>()
                {
                    fee = paid.actual_fee;
                }
            }
            _ => {}
        }
    }

    (success, error_module, error_name, fee)
}

/// Best-effort `(pallet_name, error_variant_name)` lookup for a
/// `DispatchError::Module`. Returns `(None, None)` for any other
/// dispatch variant or when the metadata can't resolve one of the
/// indices — the caller treats that as "known failed, reason opaque"
/// and leaves the columns `NULL`.
fn resolve_module_error(
    err: &allfeat::runtime_types::sp_runtime::DispatchError,
    metadata: &subxt::Metadata,
) -> (Option<String>, Option<String>) {
    use allfeat::runtime_types::sp_runtime::DispatchError as E;
    let E::Module(m) = err else {
        return (None, None);
    };
    let Some(pallet) = metadata.pallet_by_error_index(m.index) else {
        return (None, None);
    };
    // `ModuleError::error` is a 4-byte padded buffer: the first byte
    // is the variant index, the rest are reserved for future nested
    // error encodings we don't use yet.
    let Some(variant) = pallet.error_variant_by_index(m.error[0]) else {
        return (Some(pallet.name().to_string()), None);
    };
    (Some(pallet.name().to_string()), Some(variant.name.clone()))
}

/// Decode the signer address bytes into the raw 32-byte AccountId32 for
/// the `Id` variant of `MultiAddress`, the only variant the explorer
/// treats as a real account. Returns `None` for unsigned extrinsics and
/// for exotic variants (`Raw`, `Index`, `Address32`, `Address20`, …) —
/// we won't synthesise a byte pattern the UI can't meaningfully render.
fn decode_signer_bytes(addr_bytes: Option<&[u8]>) -> Option<[u8; 32]> {
    use subxt::ext::codec::Decode;
    use subxt::utils::{AccountId32, MultiAddress};
    let bytes = addr_bytes?;
    let addr = MultiAddress::<AccountId32, u32>::decode(&mut &bytes[..]).ok()?;
    match addr {
        MultiAddress::Id(id) => {
            let mut out = [0u8; 32];
            out.copy_from_slice(id.as_ref());
            Some(out)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use subxt::ext::codec::Encode;
    use subxt::utils::{AccountId32, MultiAddress};

    /// The `Id` variant is the only one we decode — this is the happy
    /// path the vast majority of signed extrinsics hit.
    #[test]
    fn decode_signer_bytes_extracts_id_variant() {
        let expected = AccountId32::from([0x17u8; 32]);
        let addr: MultiAddress<AccountId32, u32> = MultiAddress::Id(expected);
        let bytes = addr.encode();
        let got = decode_signer_bytes(Some(&bytes)).expect("Id decodes");
        assert_eq!(got, [0x17u8; 32]);
    }

    /// Unsigned extrinsics (inherents) have no address bytes at all —
    /// the projection must surface that as `None`, which maps to a
    /// NULL `signer` BYTEA column.
    #[test]
    fn decode_signer_bytes_returns_none_on_missing_address() {
        assert!(decode_signer_bytes(None).is_none());
    }

    /// `Raw` (and the other non-Id variants) are legal on-chain but
    /// we refuse to map them to AccountId32 bytes — a misleading byte
    /// pattern in the index would produce phantom hits on /account.
    #[test]
    fn decode_signer_bytes_returns_none_for_non_id_variants() {
        let addr: MultiAddress<AccountId32, u32> = MultiAddress::Raw(vec![1, 2, 3]);
        let bytes = addr.encode();
        assert!(decode_signer_bytes(Some(&bytes)).is_none());
    }

    /// Malformed address bytes shouldn't crash the whole block
    /// projection — treat the signer as absent and keep indexing.
    #[test]
    fn decode_signer_bytes_returns_none_on_malformed_bytes() {
        assert!(decode_signer_bytes(Some(&[0xff])).is_none());
        assert!(decode_signer_bytes(Some(&[])).is_none());
    }

    /// Lock the struct shape: every byte column is `[u8; 32]` or
    /// `Vec<u8>`, nothing strings-in-disguise. A future refactor that
    /// silently drops a byte column (or adds a hex one) fails here.
    #[test]
    fn row_fields_are_plain_bytes() {
        let row = ExtrinsicRow {
            block_num: 100,
            idx: 2,
            hash: [7u8; 32],
            pallet: "Balances".into(),
            call: "transfer_keep_alive".into(),
            signer: Some([9u8; 32]),
            tip: Some(1_000),
            fee: 125_000_000,
            nonce: Some(42),
            success: true,
            error_module: None,
            error_name: None,
            args_scale: vec![0x01, 0x02, 0x03],
        };
        assert_eq!(row.hash.len(), 32);
        assert_eq!(row.signer.unwrap().len(), 32);
        assert!(row.success);
        assert_eq!(row.args_scale, vec![1, 2, 3]);
    }

    /// Guard the "failed with a non-Module dispatch" branch: we flip
    /// `success = false` but leave `error_module` / `error_name` both
    /// `None` because there's no pallet/variant to resolve. Locks the
    /// contract the queries layer relies on — a NULL `error_module`
    /// column means "failed for an opaque reason", not "no failure".
    #[test]
    fn resolve_module_error_returns_none_for_non_module_variants() {
        use allfeat::runtime_types::sp_runtime::DispatchError as E;

        let metadata = load_test_metadata();
        // `BadOrigin` is a unit variant — no data to resolve.
        let (m, n) = resolve_module_error(&E::BadOrigin, &metadata);
        assert!(m.is_none() && n.is_none());
    }

    /// Load the same metadata blob the projection uses at runtime.
    /// `decode_from` tolerates all `frame_metadata` prefixed/unprefixed
    /// encodings, matching what `#[subxt::subxt(runtime_metadata_path
    /// = "...")]` consumes at macro time.
    fn load_test_metadata() -> subxt::Metadata {
        const BLOB: &[u8] = include_bytes!("../../../artifacts/allfeat-metadata.scale");
        subxt::Metadata::decode_from(BLOB).expect("metadata decodes")
    }
}
