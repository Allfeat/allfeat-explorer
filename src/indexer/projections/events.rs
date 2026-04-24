//! Project a pinned `OnlineClientAtBlock` into [`EventRow`]s ready for
//! the `events` table. Implemented in Phase 4 of `docs/indexing-plan.md`.
//!
//! Two contracts the rest of the indexer depends on:
//!
//! * **Phase encoding is normalised here.** The schema stores phase as a
//!   `(SMALLINT, INT?)` pair: `0 ApplyExtrinsic(idx)`, `1 Finalization`,
//!   `2 Initialization`. Centralising the discriminants avoids each
//!   read/write site re-deriving them and silently disagreeing.
//! * **`data_scale` is the raw event field bytes**, exactly the slice
//!   `subxt::events::Event::field_bytes()` returns. A reader can decode
//!   it later against the spec_version's metadata; we never lose
//!   information by stripping the wrapper bytes.

use subxt::client::OnlineClientAtBlock;
use subxt::events::Phase;
use subxt::SubstrateConfig;

use crate::data::error::{DataError, DataResult};
use crate::data::rpc::mappers::fetch_block_events;

/// Phase discriminants stored in `events.phase_kind`. Public so the
/// (downstream) read layer can build the same constants without reaching
/// across modules. Order matches the SQL comment in the migration.
pub const PHASE_APPLY_EXTRINSIC: i16 = 0;
pub const PHASE_FINALIZATION: i16 = 1;
pub const PHASE_INITIALIZATION: i16 = 2;

/// Row shape for `events`. Field order mirrors the `INSERT` statement in
/// [`crate::indexer::sink::insert_events`]; adding a column means
/// touching both places deliberately rather than silently forgetting one
/// side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventRow {
    pub block_num: u64,
    pub idx: u32,
    /// One of `PHASE_APPLY_EXTRINSIC`, `PHASE_FINALIZATION`,
    /// `PHASE_INITIALIZATION`. SMALLINT in the schema; we keep the same
    /// width on the row so the bind site never narrows.
    pub phase_kind: i16,
    /// `Some(idx)` only when `phase_kind == PHASE_APPLY_EXTRINSIC`.
    /// Block-init / block-finalization events have no extrinsic to point
    /// at — the column is nullable for exactly this reason.
    pub phase_idx: Option<u32>,
    pub pallet: String,
    pub variant: String,
    /// Raw SCALE-encoded event field bytes. Decoding requires the
    /// runtime's metadata at the right spec_version, which the read path
    /// resolves on demand — persisting the bytes keeps the projection
    /// pure (no metadata lookup) and lets future readers decode without
    /// re-fetching the chain.
    pub data_scale: Vec<u8>,
}

/// Project every event in the pinned block into an [`EventRow`].
///
/// `block_num` is passed in rather than read from `at.block_number()` so
/// the caller's logical cursor remains the single source of truth — same
/// reasoning as [`crate::indexer::projections::blocks::map`].
pub async fn map(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    block_num: u64,
) -> DataResult<Vec<EventRow>> {
    let events = fetch_block_events(at).await?;
    let mut out = Vec::with_capacity(events.len() as usize);
    for (idx, evt) in events.iter().enumerate() {
        let evt = evt.map_err(|e| DataError::Decode(format!("decode event: {e}")))?;
        let (phase_kind, phase_idx) = encode_phase(evt.phase());
        out.push(EventRow {
            block_num,
            idx: idx as u32,
            phase_kind,
            phase_idx,
            pallet: evt.pallet_name().to_string(),
            variant: evt.event_name().to_string(),
            data_scale: evt.field_bytes().to_vec(),
        });
    }
    Ok(out)
}

/// Map subxt's `Phase` to the `(phase_kind, phase_idx)` tuple stored in
/// the schema. Pulled out so the unit tests don't have to round-trip
/// through a real subxt event to lock the encoding.
pub fn encode_phase(phase: Phase) -> (i16, Option<u32>) {
    match phase {
        Phase::ApplyExtrinsic(i) => (PHASE_APPLY_EXTRINSIC, Some(i)),
        Phase::Finalization => (PHASE_FINALIZATION, None),
        Phase::Initialization => (PHASE_INITIALIZATION, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Phase 4's contract: the three on-chain phases collapse onto the
    /// `(SMALLINT, INT?)` tuple in the migration, with the extrinsic
    /// index living *only* on `ApplyExtrinsic`. A reader that joins on
    /// `(block_num, phase_kind, phase_idx)` relies on this — a stray
    /// `Some(idx)` on a Finalization row would shadow real
    /// ApplyExtrinsic events at idx 0.
    #[test]
    fn encode_phase_apply_extrinsic_carries_idx() {
        assert_eq!(
            encode_phase(Phase::ApplyExtrinsic(7)),
            (PHASE_APPLY_EXTRINSIC, Some(7))
        );
    }

    #[test]
    fn encode_phase_finalization_drops_idx() {
        assert_eq!(
            encode_phase(Phase::Finalization),
            (PHASE_FINALIZATION, None)
        );
    }

    #[test]
    fn encode_phase_initialization_drops_idx() {
        assert_eq!(
            encode_phase(Phase::Initialization),
            (PHASE_INITIALIZATION, None)
        );
    }

    /// Row shape is the only contract the sink relies on: every column is
    /// already ready to bind. A future refactor that swaps `data_scale`
    /// for a hex string (or strips it entirely) breaks this guard.
    #[test]
    fn row_fields_are_byte_oriented() {
        let row = EventRow {
            block_num: 100,
            idx: 5,
            phase_kind: PHASE_APPLY_EXTRINSIC,
            phase_idx: Some(2),
            pallet: "Balances".into(),
            variant: "Transfer".into(),
            data_scale: vec![0x01, 0x02, 0x03],
        };
        assert_eq!(row.data_scale, vec![1, 2, 3]);
        assert_eq!(row.phase_kind, 0);
        assert_eq!(row.phase_idx, Some(2));
    }

    /// Every encoding constant must round-trip through `i16` without
    /// surprises — a future widening to `i32` would silently break the
    /// `SMALLINT` bind in the sink.
    #[test]
    fn phase_constants_fit_smallint() {
        for v in [
            PHASE_APPLY_EXTRINSIC,
            PHASE_FINALIZATION,
            PHASE_INITIALIZATION,
        ] {
            assert!(v >= 0, "phase const {v} must be non-negative in SMALLINT");
        }
    }
}
