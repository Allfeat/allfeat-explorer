//! Extrinsic mapping: canonical `"<block>-<index>"` ids, args decoding, and
//! the per-block walk that translates subxt extrinsics to [`crate::domain::Extrinsic`].

use subxt::client::OnlineClientAtBlock;
use subxt::events::Event;
use subxt::SubstrateConfig;

use crate::data::error::{DataError, DataResult};
use crate::data::metadata::decode_call_args;
use crate::data::rpc::client::with_timeout;
use crate::data::rpc::runtime::{allfeat, melodie};
use crate::domain::{CallResult, EventRef, Extrinsic, ExtrinsicArgs};
use crate::network::RuntimeKind;

use super::common::{decode_signer_ss58, hash_string, EventsByPhase};

/// `"<block>-<index>"` — the canonical id the explorer uses everywhere.
pub fn extrinsic_id(block: u64, index: u32) -> String {
    format!("{block}-{index}")
}

/// Parse the canonical `"<block>-<index>"` id. Returns `None` if the shape is
/// wrong or either component fails to parse.
pub fn parse_extrinsic_id(id: &str) -> Option<(u64, u32)> {
    let (b, i) = id.split_once('-')?;
    Some((b.parse().ok()?, i.parse().ok()?))
}

/// Map every extrinsic in the current block to a [`crate::domain::Extrinsic`].
///
/// `timestamp_ms` is the block's wall-clock instant (already fetched by the
/// caller for the block mapper — passing it in avoids a second storage read).
/// `events_by_phase` is the block's pre-indexed event set (see
/// [`super::common::index_events_by_phase`]); sharing it across `map_extrinsics`
/// and `map_transfers` folds the previous N-per-extrinsic `ext.events().await`
/// chain into a single block-level fetch.
pub async fn map_extrinsics<'a>(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    timestamp_ms: i64,
    events_by_phase: &EventsByPhase<'a>,
    network_id: &str,
    runtime_kind: RuntimeKind,
    ss58_prefix: u16,
) -> DataResult<Vec<Extrinsic>> {
    let block_number = at.block_number();
    let raw = with_timeout("map_extrinsics_fetch", async {
        at.extrinsics()
            .fetch()
            .await
            .map_err(|e| DataError::Rpc(format!("fetch extrinsics: {e}")))
    })
    .await?;

    let empty: Vec<Event<'a, SubstrateConfig>> = Vec::new();
    let mut out = Vec::with_capacity(raw.len());
    for ext in raw.iter() {
        let ext = ext.map_err(|e| DataError::Decode(format!("decode extrinsic: {e}")))?;
        let idx = ext.index() as u32;
        let pallet = ext.pallet_name().to_string();
        let call = ext.call_name().to_string();
        let signed = ext.is_signed();
        let hash = hash_string(&ext.hash());

        let signer = if signed {
            ext.address_bytes()
                .and_then(|b| decode_signer_ss58(b, ss58_prefix))
        } else {
            None
        };

        // Events associated with this extrinsic drive three fields: the event
        // list itself, the Success/Failed result (from `System.Extrinsic*`),
        // and the paid fee (`TransactionPayment.TransactionFeePaid`). The
        // caller already indexed the block's events by phase, so this lookup
        // is O(1) and costs nothing for extrinsics that emit no events.
        let evt_slice = events_by_phase.get(&idx).unwrap_or(&empty);
        let mut events = Vec::with_capacity(evt_slice.len());
        // Default to success and downgrade to Failed only if we see an
        // explicit System.ExtrinsicFailed. Inherents (e.g. `timestamp.set`)
        // don't always emit an ExtrinsicSuccess but do succeed when included.
        let mut result = CallResult::Success;
        let mut fee: u128 = 0;
        for evt in evt_slice {
            let pallet_name = evt.pallet_name();
            let event_name = evt.event_name();
            let fields = crate::data::metadata::decode_event_fields(
                network_id,
                pallet_name,
                event_name,
                evt.field_bytes(),
                ss58_prefix,
            );
            events.push(EventRef {
                module: pallet_name.to_string(),
                name: event_name.to_string(),
                fields,
            });
            match (pallet_name, event_name) {
                ("System", "ExtrinsicFailed") => result = CallResult::Failed,
                ("TransactionPayment", "TransactionFeePaid") => {
                    // SCALE shape is identical across runtimes (who/actual_fee/
                    // tip), but the codegen produces distinct Rust types per
                    // module — dispatch on the tag and pull `actual_fee` out
                    // before the arm boundary.
                    let actual_fee = match runtime_kind {
                        RuntimeKind::Allfeat => evt
                            .decode_fields_unchecked_as::<
                                allfeat::transaction_payment::events::TransactionFeePaid,
                            >()
                            .ok()
                            .map(|paid| paid.actual_fee),
                        RuntimeKind::Melodie => evt
                            .decode_fields_unchecked_as::<
                                melodie::transaction_payment::events::TransactionFeePaid,
                            >()
                            .ok()
                            .map(|paid| paid.actual_fee),
                    };
                    if let Some(actual_fee) = actual_fee {
                        fee = actual_fee;
                    }
                }
                _ => {}
            }
        }

        let (nonce, tip) = ext
            .transaction_extensions()
            .map(|exts| (exts.nonce().map(|n| n as u32), exts.tip().unwrap_or(0)))
            .unwrap_or((None, 0));

        let args = decode_args(&ext, network_id, ss58_prefix);

        out.push(Extrinsic {
            id: extrinsic_id(block_number, idx),
            block_number,
            index: idx,
            hash,
            module: pallet,
            call,
            signed,
            signer,
            args,
            result,
            nonce,
            tip,
            fee,
            timestamp_ms,
            events,
        });
    }
    Ok(out)
}

/// Decode the call's SCALE-encoded arguments via the static metadata.
///
/// Shared code path with the DB read layer
/// ([`crate::data::indexed::queries::extrinsics::row_to_extrinsic`]): the
/// same `(pallet, call, bytes)` triple flows through
/// [`decode_call_fields`] regardless of whether the source was a live RPC
/// pin or an indexed row. Any failure — unknown pallet/variant, truncated
/// payload, field type not resolvable — falls back to [`ExtrinsicArgs::Raw`]
/// so the Parameters tab still shows the opaque bytes.
fn decode_args<C>(
    ext: &subxt::extrinsics::Extrinsic<'_, SubstrateConfig, C>,
    network_id: &str,
    ss58_prefix: u16,
) -> ExtrinsicArgs
where
    C: subxt::client::OfflineClientAtBlockT<SubstrateConfig>,
{
    decode_call_args(
        network_id,
        ext.pallet_name(),
        ext.call_name(),
        ext.call_data_bytes(),
        ss58_prefix,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_extrinsic_id_accepts_canonical_shape() {
        assert_eq!(parse_extrinsic_id("123-4"), Some((123, 4)));
        assert_eq!(parse_extrinsic_id("0-0"), Some((0, 0)));
    }

    #[test]
    fn parse_extrinsic_id_rejects_malformed_input() {
        // Must have exactly one separator and both sides must parse.
        assert_eq!(parse_extrinsic_id("123"), None);
        assert_eq!(parse_extrinsic_id("abc-4"), None);
        assert_eq!(parse_extrinsic_id("123-x"), None);
        assert_eq!(parse_extrinsic_id(""), None);
        // `-1` would sign-flip on u64; `split_once` + `u64::from_str_radix`
        // rejects it and we bubble that up as `None`.
        assert_eq!(parse_extrinsic_id("-1-0"), None);
    }

    #[test]
    fn extrinsic_id_roundtrips_through_parse() {
        let id = extrinsic_id(987, 3);
        assert_eq!(id, "987-3");
        assert_eq!(parse_extrinsic_id(&id), Some((987, 3)));
    }
}
