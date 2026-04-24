//! Project `Ats.*` events into the Postgres rows backing
//! [`crate::indexer::sink::apply_ats`]. Phase 6 of
//! `docs/indexing-plan.md`.
//!
//! Three events drive every row the projection emits:
//!
//! * `Ats.AtsCreated { ats_id, owner, commitment, protocol_version, ... }`
//!   — seeds `ats_registry` with `version_count = 1` and inserts the
//!   initial row into `ats_versions` at `version = 0`.
//! * `Ats.AtsUpdated { ats_id, version, commitment, protocol_version, ... }`
//!   — inserts the next row into `ats_versions` and bumps
//!   `ats_registry.version_count` to `version + 1` (the pallet emits
//!   1-indexed versions after the initial `version = 0`).
//! * `Ats.AtsRevoked { ats_id, ... }` — drops the registry row; the
//!   `ON DELETE CASCADE` on `ats_versions.ats_id` wipes the history
//!   set in the same transaction so the indexer never shows a ghost
//!   entry for a revoked ATS.
//!
//! The projection is **pure**: [`project`] consumes already-decoded
//! events + `ExtrinsicRow`s to attribute each op to the right
//! `(block_num, ext_idx)` and emits a [`AtsOps`] struct the sink
//! applies atomically. The async wrapper [`project_block`] pulls the
//! block's events via subxt and runs the decode loop.
//!
//! `deposits` are **not** event-derived. The pallet stores them on the
//! `AtsRecord` storage value, which the indexer doesn't scan on every
//! block. The query layer derives a reasonable approximation
//! (`owner × VERSION_DEPOSIT × version_count`) at read time — see
//! [`crate::data::indexed::queries::ats`]. Phase 6 accepts the tradeoff:
//! the on-chain deposit policy fixes a per-version reserve, and the
//! single-owner approximation matches every real-world flow today.

use subxt::client::OnlineClientAtBlock;
use subxt::events::Phase;
use subxt::SubstrateConfig;

use crate::data::error::{DataError, DataResult};
use crate::data::rpc::mappers::fetch_block_events;
use crate::data::rpc::runtime::allfeat;

use super::extrinsics::ExtrinsicRow;

/// Row shape for one insert into `ats_registry`. Written via
/// `ON CONFLICT (id) DO UPDATE SET owner = EXCLUDED.owner, ...` so a
/// replay of the same creator event is idempotent; a later
/// `AtsUpdated` event ONLY bumps `version_count` via the separate
/// [`AtsVersionCountBump`] path so `owner` / `created_block` stay
/// pinned to the creator extrinsic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtsRegistryRow {
    pub id: u64,
    pub owner: [u8; 32],
    pub created_block: u64,
    pub created_ext_idx: u32,
    /// `1` for a fresh create. The sink UPSERTs and takes the
    /// `GREATEST` against the existing row so an out-of-order apply
    /// (backfill racing the live worker) never rewinds the count.
    pub version_count: u32,
}

/// Row shape for one insert into `ats_versions`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtsVersionRow {
    pub ats_id: u64,
    pub version: u32,
    pub block_num: u64,
    pub ext_idx: u32,
    pub commitment: [u8; 32],
    pub protocol_version: u8,
}

/// Bump `ats_registry.version_count` for a given ats_id. Emitted by
/// `AtsUpdated` events — the create path writes the count directly via
/// [`AtsRegistryRow`]. Kept as its own op so the sink can apply it via
/// `GREATEST` without needing to re-fetch the row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtsVersionCountBump {
    pub ats_id: u64,
    pub version_count: u32,
}

/// Aggregated ATS side-effects for one block. Every field is
/// independently idempotent — the sink can apply them in any order and
/// a replay never double-counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AtsOps {
    pub registry_inserts: Vec<AtsRegistryRow>,
    pub version_inserts: Vec<AtsVersionRow>,
    pub version_count_bumps: Vec<AtsVersionCountBump>,
    /// ATS ids revoked in this block. The sink DELETEs them; the
    /// `ON DELETE CASCADE` on `ats_versions.ats_id` handles the
    /// version history.
    pub revocations: Vec<u64>,
}

impl AtsOps {
    pub fn is_empty(&self) -> bool {
        self.registry_inserts.is_empty()
            && self.version_inserts.is_empty()
            && self.version_count_bumps.is_empty()
            && self.revocations.is_empty()
    }
}

/// One decoded ATS event, already attributed to its originating
/// extrinsic index. Tests construct this directly to exercise [`project`]
/// without standing up a node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodedAtsEvent {
    Created {
        ats_id: u64,
        owner: [u8; 32],
        commitment: [u8; 32],
        protocol_version: u8,
    },
    Updated {
        ats_id: u64,
        version: u32,
        commitment: [u8; 32],
        protocol_version: u8,
    },
    Revoked {
        ats_id: u64,
    },
}

/// Pure projection: turn each `(ext_idx, DecodedAtsEvent)` pair into
/// the matching [`AtsOps`] entries, attributing each to `block_num`.
/// `extrinsics` is accepted for symmetry with the other projections
/// (and in case we ever need to confirm the signer against the owner);
/// it isn't consulted today.
pub fn project(
    decoded: &[(u32, DecodedAtsEvent)],
    _extrinsics: &[ExtrinsicRow],
    block_num: u64,
) -> AtsOps {
    let mut ops = AtsOps::default();
    for (ext_idx, evt) in decoded {
        match evt {
            DecodedAtsEvent::Created {
                ats_id,
                owner,
                commitment,
                protocol_version,
            } => {
                ops.registry_inserts.push(AtsRegistryRow {
                    id: *ats_id,
                    owner: *owner,
                    created_block: block_num,
                    created_ext_idx: *ext_idx,
                    version_count: 1,
                });
                ops.version_inserts.push(AtsVersionRow {
                    ats_id: *ats_id,
                    version: 0,
                    block_num,
                    ext_idx: *ext_idx,
                    commitment: *commitment,
                    protocol_version: *protocol_version,
                });
            }
            DecodedAtsEvent::Updated {
                ats_id,
                version,
                commitment,
                protocol_version,
            } => {
                ops.version_inserts.push(AtsVersionRow {
                    ats_id: *ats_id,
                    version: *version,
                    block_num,
                    ext_idx: *ext_idx,
                    commitment: *commitment,
                    protocol_version: *protocol_version,
                });
                // `version_count` is 1-indexed in the registry (an ATS
                // with versions {0, 1, 2} has version_count = 3).
                ops.version_count_bumps.push(AtsVersionCountBump {
                    ats_id: *ats_id,
                    version_count: version.saturating_add(1),
                });
            }
            DecodedAtsEvent::Revoked { ats_id } => {
                ops.revocations.push(*ats_id);
            }
        }
    }
    ops
}

/// Async wrapper used by the live + backfill workers. Fetches the
/// block's events, decodes the `Ats.*` subset, then delegates to
/// [`project`]. Events outside the `Ats` pallet are silently skipped.
pub async fn project_block(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    extrinsics: &[ExtrinsicRow],
    block_num: u64,
) -> DataResult<AtsOps> {
    let events = fetch_block_events(at).await?;
    let mut decoded = Vec::new();
    for evt in events.iter() {
        let evt = evt.map_err(|e| DataError::Decode(format!("decode event: {e}")))?;
        // ATS ops only happen inside extrinsic application — block
        // init/finalization phases never emit Ats events. Skip them
        // defensively so a future runtime emitting one doesn't slip
        // through with a misleading `ext_idx = u32::MAX` attribution.
        let Phase::ApplyExtrinsic(ext_idx) = evt.phase() else {
            continue;
        };
        if let Some(d) = decode_ats_event(&evt)? {
            decoded.push((ext_idx, d));
        }
    }
    Ok(project(&decoded, extrinsics, block_num))
}

/// Match an event to its [`DecodedAtsEvent`]. Returns `Ok(None)` for
/// any pallet/variant outside `Ats.*`; bubbles a decode error only when
/// a *known* variant fails to decode (metadata/runtime drift worth
/// surfacing loudly).
fn decode_ats_event(
    evt: &subxt::events::Event<'_, SubstrateConfig>,
) -> DataResult<Option<DecodedAtsEvent>> {
    match (evt.pallet_name(), evt.event_name()) {
        ("Ats", "AtsCreated") => {
            let d = evt
                .decode_fields_unchecked_as::<allfeat::ats::events::AtsCreated>()
                .map_err(|e| DataError::Decode(format!("Ats.AtsCreated: {e}")))?;
            Ok(Some(DecodedAtsEvent::Created {
                ats_id: d.ats_id,
                owner: account_bytes(&d.owner),
                commitment: d.commitment,
                protocol_version: d.protocol_version,
            }))
        }
        ("Ats", "AtsUpdated") => {
            let d = evt
                .decode_fields_unchecked_as::<allfeat::ats::events::AtsUpdated>()
                .map_err(|e| DataError::Decode(format!("Ats.AtsUpdated: {e}")))?;
            Ok(Some(DecodedAtsEvent::Updated {
                ats_id: d.ats_id,
                version: d.version,
                commitment: d.commitment,
                protocol_version: d.protocol_version,
            }))
        }
        ("Ats", "AtsRevoked") => {
            let d = evt
                .decode_fields_unchecked_as::<allfeat::ats::events::AtsRevoked>()
                .map_err(|e| DataError::Decode(format!("Ats.AtsRevoked: {e}")))?;
            Ok(Some(DecodedAtsEvent::Revoked { ats_id: d.ats_id }))
        }
        _ => Ok(None),
    }
}

fn account_bytes(id: &subxt::utils::AccountId32) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(id.as_ref());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: [u8; 32] = [0xaa; 32];
    const COMMIT_A: [u8; 32] = [0x11; 32];
    const COMMIT_B: [u8; 32] = [0x22; 32];

    fn run(events: &[(u32, DecodedAtsEvent)]) -> AtsOps {
        project(events, &[], 42)
    }

    /// `AtsCreated` seeds both tables in lockstep: one registry row
    /// with `version_count = 1`, one version row at `version = 0`. A
    /// future refactor that drops either side silently makes the list
    /// view disagree with the detail view.
    #[test]
    fn created_emits_registry_and_version_zero() {
        let ops = run(&[(
            3,
            DecodedAtsEvent::Created {
                ats_id: 7,
                owner: OWNER,
                commitment: COMMIT_A,
                protocol_version: 1,
            },
        )]);
        assert_eq!(ops.registry_inserts.len(), 1);
        assert_eq!(ops.version_inserts.len(), 1);
        assert!(ops.version_count_bumps.is_empty());
        assert!(ops.revocations.is_empty());

        let reg = &ops.registry_inserts[0];
        assert_eq!(reg.id, 7);
        assert_eq!(reg.owner, OWNER);
        assert_eq!(reg.created_block, 42);
        assert_eq!(reg.created_ext_idx, 3);
        assert_eq!(reg.version_count, 1);

        let ver = &ops.version_inserts[0];
        assert_eq!(ver.ats_id, 7);
        assert_eq!(ver.version, 0);
        assert_eq!(ver.block_num, 42);
        assert_eq!(ver.ext_idx, 3);
        assert_eq!(ver.commitment, COMMIT_A);
        assert_eq!(ver.protocol_version, 1);
    }

    /// `AtsUpdated` never writes the registry row directly — the
    /// creator extrinsic owns that slot. It emits the new version plus
    /// a count bump the sink applies via `GREATEST`. Locks the
    /// invariant the read layer relies on: `owner`/`created_block`
    /// never change after creation.
    #[test]
    fn updated_emits_version_and_count_bump_only() {
        let ops = run(&[(
            5,
            DecodedAtsEvent::Updated {
                ats_id: 9,
                version: 3,
                commitment: COMMIT_B,
                protocol_version: 2,
            },
        )]);
        assert!(ops.registry_inserts.is_empty());
        assert_eq!(ops.version_inserts.len(), 1);
        assert_eq!(ops.version_count_bumps.len(), 1);
        assert!(ops.revocations.is_empty());

        let ver = &ops.version_inserts[0];
        assert_eq!(ver.ats_id, 9);
        assert_eq!(ver.version, 3);
        assert_eq!(ver.ext_idx, 5);
        assert_eq!(ver.commitment, COMMIT_B);
        assert_eq!(ver.protocol_version, 2);

        let bump = &ops.version_count_bumps[0];
        assert_eq!(bump.ats_id, 9);
        // version = 3 (0-indexed) → count = 4 (1-indexed).
        assert_eq!(bump.version_count, 4);
    }

    /// `AtsRevoked` schedules a DELETE on the registry. The sink
    /// relies on `ON DELETE CASCADE` to drop the version history, so
    /// the projection doesn't emit explicit version deletes.
    #[test]
    fn revoked_schedules_registry_delete_only() {
        let ops = run(&[(1, DecodedAtsEvent::Revoked { ats_id: 4 })]);
        assert!(ops.registry_inserts.is_empty());
        assert!(ops.version_inserts.is_empty());
        assert!(ops.version_count_bumps.is_empty());
        assert_eq!(ops.revocations, vec![4]);
    }

    /// A mixed block (create + update + revoke across three different
    /// ats_ids) must preserve each op's attribution without cross-talk.
    /// Guards against a future refactor that accidentally indexes by
    /// position instead of ats_id.
    #[test]
    fn multi_event_block_preserves_ats_id_attribution() {
        let ops = run(&[
            (
                0,
                DecodedAtsEvent::Created {
                    ats_id: 100,
                    owner: OWNER,
                    commitment: COMMIT_A,
                    protocol_version: 1,
                },
            ),
            (
                1,
                DecodedAtsEvent::Updated {
                    ats_id: 50,
                    version: 2,
                    commitment: COMMIT_B,
                    protocol_version: 1,
                },
            ),
            (2, DecodedAtsEvent::Revoked { ats_id: 7 }),
        ]);
        assert_eq!(ops.registry_inserts.len(), 1);
        assert_eq!(ops.registry_inserts[0].id, 100);
        assert_eq!(ops.version_inserts.len(), 2);
        assert_eq!(ops.version_inserts[0].ats_id, 100);
        assert_eq!(ops.version_inserts[0].version, 0);
        assert_eq!(ops.version_inserts[1].ats_id, 50);
        assert_eq!(ops.version_inserts[1].version, 2);
        assert_eq!(ops.version_count_bumps.len(), 1);
        assert_eq!(ops.version_count_bumps[0].ats_id, 50);
        assert_eq!(ops.revocations, vec![7]);
    }

    /// An empty event list leaves [`AtsOps`] empty — the sink can
    /// short-circuit on `is_empty()` and avoid issuing zero-row
    /// INSERTs on every non-ATS block (the common case on a busy
    /// chain).
    #[test]
    fn empty_events_produce_empty_ops() {
        let ops = run(&[]);
        assert!(ops.is_empty());
    }
}
