//! Map `Balances.Transfer` events into [`crate::domain::Transfer`] rows,
//! attributing each to the extrinsic that emitted it via the phase index.

use crate::data::error::{DataError, DataResult};
use crate::data::rpc::runtime::{allfeat, melodie};
use crate::domain::{Extrinsic, Transfer};
use crate::network::RuntimeKind;

use super::common::{account_ss58, EventsByPhase};

/// Find every `Balances.Transfer { from, to, amount }` event in the block and
/// zip it with the extrinsic that triggered it (via the event's phase).
///
/// Consumes the pre-indexed `events_by_phase` so no extra fetch is needed —
/// the caller already paid for one in
/// [`super::common::index_events_by_phase`] (shared with
/// [`super::extrinsics::map_extrinsics`]).
///
/// Events emitted outside an extrinsic phase (block init/finalization) would
/// in theory surface here too — those are stripped by the indexer already,
/// and transfers attributed to them wouldn't have an extrinsic id the UI can
/// link to anyway.
pub fn map_transfers(
    block_extrinsics: &[Extrinsic],
    events_by_phase: &EventsByPhase<'_>,
    runtime_kind: RuntimeKind,
    ss58_prefix: u16,
) -> DataResult<Vec<Transfer>> {
    Ok(map_transfers_with_event_idx(
        block_extrinsics,
        events_by_phase,
        runtime_kind,
        ss58_prefix,
    )?
    .into_iter()
    .map(|(t, _)| t)
    .collect())
}

/// Same semantics as [`map_transfers`], but pairs each transfer with the
/// originating event's `event_idx` (index within the block's event
/// stream). The paginated read path uses this to build a
/// [`crate::data::cursor::TransferCursor`] after the N+1 trim — the
/// plain domain [`Transfer`] type doesn't carry `event_idx`, and the
/// live subscription path doesn't need it, so the split keeps the
/// streaming producer's allocations lean.
pub fn map_transfers_with_event_idx(
    block_extrinsics: &[Extrinsic],
    events_by_phase: &EventsByPhase<'_>,
    runtime_kind: RuntimeKind,
    ss58_prefix: u16,
) -> DataResult<Vec<(Transfer, u32)>> {
    let mut out = Vec::new();
    for (idx, evt_slice) in events_by_phase {
        let Some(extrinsic) = block_extrinsics.iter().find(|e| e.index == *idx) else {
            continue;
        };
        for evt in evt_slice {
            if evt.pallet_name() != "Balances" || evt.event_name() != "Transfer" {
                continue;
            }
            // SCALE shape is identical (AccountId32, AccountId32, u128) on
            // both runtimes, but the codegen produces distinct Rust types
            // per module — dispatch on the tag and collapse into the
            // ss58/u128 trio the domain layer expects before leaving the
            // arm.
            let (from, to, amount) = match runtime_kind {
                RuntimeKind::Allfeat => {
                    let decoded = evt
                        .decode_fields_unchecked_as::<allfeat::balances::events::Transfer>()
                        .map_err(|e| {
                            DataError::Decode(format!("decode Balances.Transfer: {e}"))
                        })?;
                    (
                        account_ss58(&decoded.from, ss58_prefix),
                        account_ss58(&decoded.to, ss58_prefix),
                        decoded.amount,
                    )
                }
                RuntimeKind::Melodie => {
                    let decoded = evt
                        .decode_fields_unchecked_as::<melodie::balances::events::Transfer>()
                        .map_err(|e| {
                            DataError::Decode(format!("decode Balances.Transfer: {e}"))
                        })?;
                    (
                        account_ss58(&decoded.from, ss58_prefix),
                        account_ss58(&decoded.to, ss58_prefix),
                        decoded.amount,
                    )
                }
            };
            out.push((
                Transfer {
                    extrinsic: extrinsic.clone(),
                    from,
                    to,
                    amount,
                },
                evt.index(),
            ));
        }
    }
    // Deterministic newest-last ordering on insertion isn't guaranteed
    // because `events_by_phase` is a `HashMap`; sort by `event_idx`
    // ascending so the caller sees chain-emission order and the cursor
    // arithmetic at the call site stays predictable.
    out.sort_by_key(|(_, event_idx)| *event_idx);
    Ok(out)
}
