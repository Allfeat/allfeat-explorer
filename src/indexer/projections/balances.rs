//! Project `Balances.*` and `TransactionPayment.TransactionFeePaid`
//! events into [`BalanceMovementRow`]s ready for the
//! `balance_movements` table. Implemented in Phase 4 of
//! `docs/indexing-plan.md`.
//!
//! Two design decisions worth highlighting:
//!
//! * **`MovementKind` is the single source of truth for the SMALLINT
//!   discriminants written to Postgres.** The migration's column comment
//!   mirrors the Rust enum; updating one without the other would cause
//!   silent semantic drift on read. Tests guard the mapping
//!   ([`movement_kind_smallint_round_trip`]).
//! * **`delta` is a signed `i128`.** Stored as `NUMERIC(39)` on the wire,
//!   bound through `to_string()` so the sink avoids the
//!   `bigdecimal`/`rust_decimal` sqlx features (same trick already used
//!   for `extrinsics.fee`). Negative values fall well within
//!   `NUMERIC(39)`'s signed range — `i128` magnitude tops out at 39 digits.
//!
//! Phase 4 contract (per `docs/indexing-plan.md` §7 — "Phase 4 — Events
//! + transfers"):
//!
//! * `Balances.Transfer` produces **2 symmetric rows** — one per
//!   account, opposite signs, `counterparty` populated on both.
//! * Every other tracked event produces **1 row**, with `counterparty`
//!   left `None`. That includes `ReserveRepatriated`, even though it
//!   carries `from`/`to` — the plan explicitly opts for the single-row
//!   shape; the destination side will be reconstructed in Phase 5 when
//!   `account_balances` reconciliation lands.
//!
//! The projection is **pure**: it consumes already-decoded balance
//! events plus extrinsic rows (so the per-extrinsic `Fee` row can be
//! attributed back to its signer) and emits `BalanceMovementRow`s. The
//! async wrapper [`project_block`] does the subxt fetch + per-event
//! decode, then delegates to [`project`].

use subxt::client::OnlineClientAtBlock;
use subxt::SubstrateConfig;

use crate::data::error::{DataError, DataResult};
use crate::data::rpc::mappers::fetch_block_events;
use crate::data::rpc::runtime::{allfeat, melodie};
use crate::network::RuntimeKind;

use super::extrinsics::ExtrinsicRow;

/// Decode an event against the active runtime's codegen and run `body`
/// against the decoded value. Each runtime arm produces a distinct Rust
/// type for `$d`, but the macro body only references fields/types that
/// are subxt-shared (e.g. `AccountId32`, primitives), so the body's
/// expression has the same type in both arms and the surrounding
/// expression's type checks. Localised to this projection because no
/// other site dispatches across this many event variants — see
/// `decode_balance_event` for usage.
macro_rules! decode_event {
    ($evt:expr, $rk:expr, $pallet:ident, $variant:ident, $err:literal, |$d:ident| $body:expr) => {
        match $rk {
            RuntimeKind::Allfeat => {
                let $d = $evt
                    .decode_fields_unchecked_as::<allfeat::$pallet::events::$variant>()
                    .map_err(|e| DataError::Decode(format!(concat!($err, ": {}"), e)))?;
                $body
            }
            RuntimeKind::Melodie => {
                let $d = $evt
                    .decode_fields_unchecked_as::<melodie::$pallet::events::$variant>()
                    .map_err(|e| DataError::Decode(format!(concat!($err, ": {}"), e)))?;
                $body
            }
        }
    };
}

/// Discriminants stored in `balance_movements.kind`. Order locked to
/// the comment in `migrations/001_initial.sql` — adding a variant means
/// touching both places.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MovementKind {
    Transfer = 0,
    Deposit = 1,
    Withdraw = 2,
    Fee = 3,
    Slash = 4,
    Reserve = 5,
    Unreserve = 6,
    ReserveRepatriated = 7,
    Burn = 8,
    Minted = 9,
    Frozen = 10,
    Thawed = 11,
    Locked = 12,
    Unlocked = 13,
    Held = 14,
    Released = 15,
    BurnedHeld = 16,
    TransferAndHold = 17,
    TransferOnHold = 18,
    Endowed = 19,
    DustLost = 20,
    Restored = 21,
    Suspended = 22,
}

impl MovementKind {
    /// `i16` view bound directly into the `SMALLINT` column. A widening
    /// to `i32` here would silently truncate — keep both sides aligned.
    pub fn as_i16(self) -> i16 {
        self as i16
    }
}

/// Row shape for `balance_movements`. Field order mirrors the `INSERT`
/// statement in [`crate::indexer::sink::insert_balance_movements`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BalanceMovementRow {
    pub block_num: u64,
    pub event_idx: u32,
    pub account: [u8; 32],
    pub kind: MovementKind,
    /// Signed magnitude of the on-chain amount: positive when the
    /// account *gains* the value (Deposit / Mint / Reserve / Frozen /
    /// Locked / Transfer-recipient), negative when it *loses* (Withdraw
    /// / Fee / Slash / Burn / Unreserve / Thawed / Unlocked /
    /// Transfer-sender / ReserveRepatriated.from).
    pub delta: i128,
    /// Set to `Some(other_account)` only on the two `Transfer` rows —
    /// the schema column is nullable for exactly this reason.
    pub counterparty: Option<[u8; 32]>,
}

/// Decoded balance-affecting events, grouped by event index. The async
/// wrapper [`project_block`] decodes the raw subxt events into this
/// shape; tests construct it directly to exercise the mapping without
/// standing up a node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodedBalanceEvent {
    Transfer {
        from: [u8; 32],
        to: [u8; 32],
        amount: u128,
    },
    Deposit {
        who: [u8; 32],
        amount: u128,
    },
    Withdraw {
        who: [u8; 32],
        amount: u128,
    },
    Slash {
        who: [u8; 32],
        amount: u128,
    },
    Reserve {
        who: [u8; 32],
        amount: u128,
    },
    Unreserve {
        who: [u8; 32],
        amount: u128,
    },
    ReserveRepatriated {
        from: [u8; 32],
        amount: u128,
    },
    Burn {
        who: [u8; 32],
        amount: u128,
    },
    Mint {
        who: [u8; 32],
        amount: u128,
    },
    Frozen {
        who: [u8; 32],
        amount: u128,
    },
    Thawed {
        who: [u8; 32],
        amount: u128,
    },
    Locked {
        who: [u8; 32],
        amount: u128,
    },
    Unlocked {
        who: [u8; 32],
        amount: u128,
    },
    /// `TransactionPayment.TransactionFeePaid { who, actual_fee }`.
    /// The fee row uses `actual_fee` as the magnitude (which already
    /// includes any tip) and is attributed back to the extrinsic
    /// signer via [`project`].
    Fee {
        who: [u8; 32],
        amount: u128,
    },
    /// Modern `fungible::hold` events emitted by pallets that use
    /// `Mutate::hold` / `release` (TokenAllocation, Preimage, Session,
    /// Ats). The hold adds to `reserved`; the release / burn subtracts
    /// from it. `reason` is discarded at the projection layer — the
    /// `Balances.*` kind discriminates well enough for the activity feed.
    Held {
        who: [u8; 32],
        amount: u128,
    },
    Released {
        who: [u8; 32],
        amount: u128,
    },
    BurnedHeld {
        who: [u8; 32],
        amount: u128,
    },
    /// `Balances.TransferAndHold { source, dest, transferred, reason }` —
    /// source loses `transferred` free; dest gains it as reserved.
    TransferAndHold {
        source: [u8; 32],
        dest: [u8; 32],
        amount: u128,
    },
    /// `Balances.TransferOnHold { source, dest, amount, reason }` —
    /// held funds move from source to dest, both on the reserved side.
    TransferOnHold {
        source: [u8; 32],
        dest: [u8; 32],
        amount: u128,
    },
    Endowed {
        account: [u8; 32],
        amount: u128,
    },
    DustLost {
        account: [u8; 32],
        amount: u128,
    },
    Restored {
        who: [u8; 32],
        amount: u128,
    },
    Suspended {
        who: [u8; 32],
        amount: u128,
    },
}

/// Pure projection: turn each `(event_idx, DecodedBalanceEvent)` pair
/// into the matching balance-movement rows, attributing each row to
/// `block_num`. `extrinsics` is reserved for future cross-checks (e.g.
/// confirming a `Fee` row's signer matches the extrinsic that emitted
/// the event); it isn't consulted yet, but lives in the signature so
/// the live worker call site doesn't change when the cross-check lands.
pub fn project(
    decoded: &[(u32, DecodedBalanceEvent)],
    _extrinsics: &[ExtrinsicRow],
    block_num: u64,
) -> Vec<BalanceMovementRow> {
    let mut out = Vec::with_capacity(decoded.len());
    for (event_idx, evt) in decoded {
        match evt {
            DecodedBalanceEvent::Transfer { from, to, amount } => {
                // Transfer is the one event the schema treats as
                // bidirectional: the `(block_num, event_idx, account)`
                // PK admits both rows because the third component
                // differs.
                let amount = *amount;
                out.push(BalanceMovementRow {
                    block_num,
                    event_idx: *event_idx,
                    account: *from,
                    kind: MovementKind::Transfer,
                    delta: -(amount as i128),
                    counterparty: Some(*to),
                });
                out.push(BalanceMovementRow {
                    block_num,
                    event_idx: *event_idx,
                    account: *to,
                    kind: MovementKind::Transfer,
                    delta: amount as i128,
                    counterparty: Some(*from),
                });
            }
            DecodedBalanceEvent::Deposit { who, amount } => {
                out.push(single(
                    block_num,
                    *event_idx,
                    *who,
                    MovementKind::Deposit,
                    *amount as i128,
                ));
            }
            DecodedBalanceEvent::Withdraw { who, amount } => {
                out.push(single(
                    block_num,
                    *event_idx,
                    *who,
                    MovementKind::Withdraw,
                    -(*amount as i128),
                ));
            }
            DecodedBalanceEvent::Slash { who, amount } => {
                out.push(single(
                    block_num,
                    *event_idx,
                    *who,
                    MovementKind::Slash,
                    -(*amount as i128),
                ));
            }
            DecodedBalanceEvent::Reserve { who, amount } => {
                out.push(single(
                    block_num,
                    *event_idx,
                    *who,
                    MovementKind::Reserve,
                    *amount as i128,
                ));
            }
            DecodedBalanceEvent::Unreserve { who, amount } => {
                out.push(single(
                    block_num,
                    *event_idx,
                    *who,
                    MovementKind::Unreserve,
                    -(*amount as i128),
                ));
            }
            DecodedBalanceEvent::ReserveRepatriated { from, amount } => {
                // Single-row shape per Phase 4 contract: only the source
                // side is recorded. Phase 5 will revisit when account
                // reconciliation needs to track the destination too.
                out.push(single(
                    block_num,
                    *event_idx,
                    *from,
                    MovementKind::ReserveRepatriated,
                    -(*amount as i128),
                ));
            }
            DecodedBalanceEvent::Burn { who, amount } => {
                out.push(single(
                    block_num,
                    *event_idx,
                    *who,
                    MovementKind::Burn,
                    -(*amount as i128),
                ));
            }
            DecodedBalanceEvent::Mint { who, amount } => {
                out.push(single(
                    block_num,
                    *event_idx,
                    *who,
                    MovementKind::Minted,
                    *amount as i128,
                ));
            }
            DecodedBalanceEvent::Frozen { who, amount } => {
                out.push(single(
                    block_num,
                    *event_idx,
                    *who,
                    MovementKind::Frozen,
                    *amount as i128,
                ));
            }
            DecodedBalanceEvent::Thawed { who, amount } => {
                out.push(single(
                    block_num,
                    *event_idx,
                    *who,
                    MovementKind::Thawed,
                    -(*amount as i128),
                ));
            }
            DecodedBalanceEvent::Locked { who, amount } => {
                out.push(single(
                    block_num,
                    *event_idx,
                    *who,
                    MovementKind::Locked,
                    *amount as i128,
                ));
            }
            DecodedBalanceEvent::Unlocked { who, amount } => {
                out.push(single(
                    block_num,
                    *event_idx,
                    *who,
                    MovementKind::Unlocked,
                    -(*amount as i128),
                ));
            }
            DecodedBalanceEvent::Fee { who, amount } => {
                out.push(single(
                    block_num,
                    *event_idx,
                    *who,
                    MovementKind::Fee,
                    -(*amount as i128),
                ));
            }
            DecodedBalanceEvent::Held { who, amount } => {
                out.push(single(
                    block_num,
                    *event_idx,
                    *who,
                    MovementKind::Held,
                    *amount as i128,
                ));
            }
            DecodedBalanceEvent::Released { who, amount } => {
                out.push(single(
                    block_num,
                    *event_idx,
                    *who,
                    MovementKind::Released,
                    -(*amount as i128),
                ));
            }
            DecodedBalanceEvent::BurnedHeld { who, amount } => {
                out.push(single(
                    block_num,
                    *event_idx,
                    *who,
                    MovementKind::BurnedHeld,
                    -(*amount as i128),
                ));
            }
            DecodedBalanceEvent::TransferAndHold {
                source,
                dest,
                amount,
            } => {
                let amount = *amount;
                out.push(BalanceMovementRow {
                    block_num,
                    event_idx: *event_idx,
                    account: *source,
                    kind: MovementKind::TransferAndHold,
                    delta: -(amount as i128),
                    counterparty: Some(*dest),
                });
                out.push(BalanceMovementRow {
                    block_num,
                    event_idx: *event_idx,
                    account: *dest,
                    kind: MovementKind::TransferAndHold,
                    delta: amount as i128,
                    counterparty: Some(*source),
                });
            }
            DecodedBalanceEvent::TransferOnHold {
                source,
                dest,
                amount,
            } => {
                let amount = *amount;
                out.push(BalanceMovementRow {
                    block_num,
                    event_idx: *event_idx,
                    account: *source,
                    kind: MovementKind::TransferOnHold,
                    delta: -(amount as i128),
                    counterparty: Some(*dest),
                });
                out.push(BalanceMovementRow {
                    block_num,
                    event_idx: *event_idx,
                    account: *dest,
                    kind: MovementKind::TransferOnHold,
                    delta: amount as i128,
                    counterparty: Some(*source),
                });
            }
            DecodedBalanceEvent::Endowed { account, amount } => {
                out.push(single(
                    block_num,
                    *event_idx,
                    *account,
                    MovementKind::Endowed,
                    *amount as i128,
                ));
            }
            DecodedBalanceEvent::DustLost { account, amount } => {
                out.push(single(
                    block_num,
                    *event_idx,
                    *account,
                    MovementKind::DustLost,
                    -(*amount as i128),
                ));
            }
            DecodedBalanceEvent::Restored { who, amount } => {
                out.push(single(
                    block_num,
                    *event_idx,
                    *who,
                    MovementKind::Restored,
                    *amount as i128,
                ));
            }
            DecodedBalanceEvent::Suspended { who, amount } => {
                out.push(single(
                    block_num,
                    *event_idx,
                    *who,
                    MovementKind::Suspended,
                    -(*amount as i128),
                ));
            }
        }
    }
    out
}

/// Output of [`project_block`]: the movement rows we want to insert,
/// plus the superset of accounts touched by any balance-affecting event
/// in the block. The latter is what the snapshot pipeline consumes — it
/// has to stay comprehensive even when we don't model an event as a
/// movement (e.g. `Balances.BalanceSet`, `Balances.Upgraded`) so no
/// account silently disappears from `account_balances`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockBalanceProjection {
    pub movements: Vec<BalanceMovementRow>,
    pub touched_accounts: Vec<[u8; 32]>,
}

/// Async wrapper used by the live + backfill workers. Fetches the
/// block's events once, then does two things per event: collect every
/// account the event mentions (into `touched_accounts`) and, if the
/// event has clear delta semantics, emit a [`BalanceMovementRow`].
/// Events outside the `Balances`/`TransactionPayment` pallets are
/// silently skipped.
pub async fn project_block(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    extrinsics: &[ExtrinsicRow],
    block_num: u64,
    runtime_kind: RuntimeKind,
) -> DataResult<BlockBalanceProjection> {
    let events = fetch_block_events(at).await?;
    let mut decoded = Vec::new();
    let mut touched_accounts: Vec<[u8; 32]> = Vec::new();
    for (idx, evt) in events.iter().enumerate() {
        let evt = evt.map_err(|e| DataError::Decode(format!("decode event: {e}")))?;
        // Process events from every phase, not just `ApplyExtrinsic`.
        // `pallet-token-allocation` auto-distributes vested funds in
        // `on_initialize` at each epoch boundary; the resulting
        // `Balances::Released` events sit in `Phase::Initialization`.
        // Skipping them left the beneficiary out of `touched_accounts`,
        // so the snapshot pipeline never refetched `System::Account` and
        // the DB kept the genesis-state `free`/`reserved` indefinitely.
        // `event_idx` is the event's position in the block-wide vector,
        // so it stays a unique key regardless of which phase emitted it.
        if let Some(d) = decode_balance_event(&evt, &mut touched_accounts, runtime_kind)? {
            decoded.push((idx as u32, d));
        }
    }
    Ok(BlockBalanceProjection {
        movements: project(&decoded, extrinsics, block_num),
        touched_accounts,
    })
}

/// Match an event to its `DecodedBalanceEvent` and, as a side effect,
/// push every account the event mentions onto `touched`. The two jobs
/// are coupled on purpose: any arm that decodes fresh accounts must
/// contribute them to the snapshot set, or the `account_balances` row
/// will drift until the address shows up in another event.
///
/// Returns `Ok(None)` for:
///   * events we recognise but choose not to model as movements
///     (`BalanceSet`, `Upgraded` — both still push their `who` onto
///     `touched`);
///   * pallet/variants we don't track at all (global-issuance events
///     like `Issued`/`Rescinded` that carry no account, and everything
///     outside the `Balances`/`TransactionPayment` pallets).
///
/// Bubbles a decode error only when a *known* variant fails to decode
/// — that would mean the metadata drifted out from under the
/// projection and is worth surfacing loudly.
fn decode_balance_event(
    evt: &subxt::events::Event<'_, SubstrateConfig>,
    touched: &mut Vec<[u8; 32]>,
    runtime_kind: RuntimeKind,
) -> DataResult<Option<DecodedBalanceEvent>> {
    match (evt.pallet_name(), evt.event_name()) {
        ("Balances", "Transfer") => {
            decode_event!(evt, runtime_kind, balances, Transfer, "Balances.Transfer", |d| {
                let from = account_bytes(&d.from);
                let to = account_bytes(&d.to);
                touched.push(from);
                touched.push(to);
                Ok(Some(DecodedBalanceEvent::Transfer {
                    from,
                    to,
                    amount: d.amount,
                }))
            })
        }
        ("Balances", "Deposit") => {
            decode_event!(evt, runtime_kind, balances, Deposit, "Balances.Deposit", |d| {
                let who = account_bytes(&d.who);
                touched.push(who);
                Ok(Some(DecodedBalanceEvent::Deposit {
                    who,
                    amount: d.amount,
                }))
            })
        }
        ("Balances", "Withdraw") => {
            decode_event!(evt, runtime_kind, balances, Withdraw, "Balances.Withdraw", |d| {
                let who = account_bytes(&d.who);
                touched.push(who);
                Ok(Some(DecodedBalanceEvent::Withdraw {
                    who,
                    amount: d.amount,
                }))
            })
        }
        ("Balances", "Slashed") => {
            decode_event!(evt, runtime_kind, balances, Slashed, "Balances.Slashed", |d| {
                let who = account_bytes(&d.who);
                touched.push(who);
                Ok(Some(DecodedBalanceEvent::Slash {
                    who,
                    amount: d.amount,
                }))
            })
        }
        ("Balances", "Reserved") => {
            decode_event!(evt, runtime_kind, balances, Reserved, "Balances.Reserved", |d| {
                let who = account_bytes(&d.who);
                touched.push(who);
                Ok(Some(DecodedBalanceEvent::Reserve {
                    who,
                    amount: d.amount,
                }))
            })
        }
        ("Balances", "Unreserved") => {
            decode_event!(evt, runtime_kind, balances, Unreserved, "Balances.Unreserved", |d| {
                let who = account_bytes(&d.who);
                touched.push(who);
                Ok(Some(DecodedBalanceEvent::Unreserve {
                    who,
                    amount: d.amount,
                }))
            })
        }
        ("Balances", "ReserveRepatriated") => {
            decode_event!(
                evt,
                runtime_kind,
                balances,
                ReserveRepatriated,
                "Balances.ReserveRepatriated",
                |d| {
                    let from = account_bytes(&d.from);
                    touched.push(from);
                    Ok(Some(DecodedBalanceEvent::ReserveRepatriated {
                        from,
                        amount: d.amount,
                    }))
                }
            )
        }
        ("Balances", "Burned") => {
            decode_event!(evt, runtime_kind, balances, Burned, "Balances.Burned", |d| {
                let who = account_bytes(&d.who);
                touched.push(who);
                Ok(Some(DecodedBalanceEvent::Burn {
                    who,
                    amount: d.amount,
                }))
            })
        }
        ("Balances", "Minted") => {
            decode_event!(evt, runtime_kind, balances, Minted, "Balances.Minted", |d| {
                let who = account_bytes(&d.who);
                touched.push(who);
                Ok(Some(DecodedBalanceEvent::Mint {
                    who,
                    amount: d.amount,
                }))
            })
        }
        ("Balances", "Frozen") => {
            decode_event!(evt, runtime_kind, balances, Frozen, "Balances.Frozen", |d| {
                let who = account_bytes(&d.who);
                touched.push(who);
                Ok(Some(DecodedBalanceEvent::Frozen {
                    who,
                    amount: d.amount,
                }))
            })
        }
        ("Balances", "Thawed") => {
            decode_event!(evt, runtime_kind, balances, Thawed, "Balances.Thawed", |d| {
                let who = account_bytes(&d.who);
                touched.push(who);
                Ok(Some(DecodedBalanceEvent::Thawed {
                    who,
                    amount: d.amount,
                }))
            })
        }
        ("Balances", "Locked") => {
            decode_event!(evt, runtime_kind, balances, Locked, "Balances.Locked", |d| {
                let who = account_bytes(&d.who);
                touched.push(who);
                Ok(Some(DecodedBalanceEvent::Locked {
                    who,
                    amount: d.amount,
                }))
            })
        }
        ("Balances", "Unlocked") => {
            decode_event!(evt, runtime_kind, balances, Unlocked, "Balances.Unlocked", |d| {
                let who = account_bytes(&d.who);
                touched.push(who);
                Ok(Some(DecodedBalanceEvent::Unlocked {
                    who,
                    amount: d.amount,
                }))
            })
        }
        ("Balances", "Held") => {
            decode_event!(evt, runtime_kind, balances, Held, "Balances.Held", |d| {
                let who = account_bytes(&d.who);
                touched.push(who);
                Ok(Some(DecodedBalanceEvent::Held {
                    who,
                    amount: d.amount,
                }))
            })
        }
        ("Balances", "Released") => {
            decode_event!(evt, runtime_kind, balances, Released, "Balances.Released", |d| {
                let who = account_bytes(&d.who);
                touched.push(who);
                Ok(Some(DecodedBalanceEvent::Released {
                    who,
                    amount: d.amount,
                }))
            })
        }
        ("Balances", "BurnedHeld") => {
            decode_event!(evt, runtime_kind, balances, BurnedHeld, "Balances.BurnedHeld", |d| {
                let who = account_bytes(&d.who);
                touched.push(who);
                Ok(Some(DecodedBalanceEvent::BurnedHeld {
                    who,
                    amount: d.amount,
                }))
            })
        }
        ("Balances", "TransferAndHold") => {
            decode_event!(
                evt,
                runtime_kind,
                balances,
                TransferAndHold,
                "Balances.TransferAndHold",
                |d| {
                    let source = account_bytes(&d.source);
                    let dest = account_bytes(&d.dest);
                    touched.push(source);
                    touched.push(dest);
                    Ok(Some(DecodedBalanceEvent::TransferAndHold {
                        source,
                        dest,
                        amount: d.transferred,
                    }))
                }
            )
        }
        ("Balances", "TransferOnHold") => {
            decode_event!(
                evt,
                runtime_kind,
                balances,
                TransferOnHold,
                "Balances.TransferOnHold",
                |d| {
                    let source = account_bytes(&d.source);
                    let dest = account_bytes(&d.dest);
                    touched.push(source);
                    touched.push(dest);
                    Ok(Some(DecodedBalanceEvent::TransferOnHold {
                        source,
                        dest,
                        amount: d.amount,
                    }))
                }
            )
        }
        ("Balances", "Endowed") => {
            decode_event!(evt, runtime_kind, balances, Endowed, "Balances.Endowed", |d| {
                let account = account_bytes(&d.account);
                touched.push(account);
                Ok(Some(DecodedBalanceEvent::Endowed {
                    account,
                    amount: d.free_balance,
                }))
            })
        }
        ("Balances", "DustLost") => {
            decode_event!(evt, runtime_kind, balances, DustLost, "Balances.DustLost", |d| {
                let account = account_bytes(&d.account);
                touched.push(account);
                Ok(Some(DecodedBalanceEvent::DustLost {
                    account,
                    amount: d.amount,
                }))
            })
        }
        ("Balances", "Restored") => {
            decode_event!(evt, runtime_kind, balances, Restored, "Balances.Restored", |d| {
                let who = account_bytes(&d.who);
                touched.push(who);
                Ok(Some(DecodedBalanceEvent::Restored {
                    who,
                    amount: d.amount,
                }))
            })
        }
        ("Balances", "Suspended") => {
            decode_event!(evt, runtime_kind, balances, Suspended, "Balances.Suspended", |d| {
                let who = account_bytes(&d.who);
                touched.push(who);
                Ok(Some(DecodedBalanceEvent::Suspended {
                    who,
                    amount: d.amount,
                }))
            })
        }
        ("Balances", "BalanceSet") => {
            // Root override — delta is unknowable without the prior
            // balance, so we don't emit a movement row. The snapshot
            // pipeline picks up the new value via `touched`.
            decode_event!(evt, runtime_kind, balances, BalanceSet, "Balances.BalanceSet", |d| {
                touched.push(account_bytes(&d.who));
                Ok(None)
            })
        }
        ("Balances", "Upgraded") => {
            // Account storage migration (no balance change, but the
            // System::Account row flips shape and the snapshot is a
            // good time to re-read it).
            decode_event!(evt, runtime_kind, balances, Upgraded, "Balances.Upgraded", |d| {
                touched.push(account_bytes(&d.who));
                Ok(None)
            })
        }
        ("TransactionPayment", "TransactionFeePaid") => {
            decode_event!(
                evt,
                runtime_kind,
                transaction_payment,
                TransactionFeePaid,
                "TransactionPayment.TransactionFeePaid",
                |d| {
                    let who = account_bytes(&d.who);
                    touched.push(who);
                    Ok(Some(DecodedBalanceEvent::Fee {
                        who,
                        amount: d.actual_fee,
                    }))
                }
            )
        }
        _ => Ok(None),
    }
}

fn single(
    block_num: u64,
    event_idx: u32,
    account: [u8; 32],
    kind: MovementKind,
    delta: i128,
) -> BalanceMovementRow {
    BalanceMovementRow {
        block_num,
        event_idx,
        account,
        kind,
        delta,
        counterparty: None,
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

    const A: [u8; 32] = [0xaa; 32];
    const B: [u8; 32] = [0xbb; 32];
    const C: [u8; 32] = [0xcc; 32];

    fn one(events: &[(u32, DecodedBalanceEvent)]) -> Vec<BalanceMovementRow> {
        project(events, &[], 100)
    }

    /// Plan contract: a `Transfer` produces exactly two rows whose
    /// deltas cancel. Both rows share the event index (the PK admits
    /// them because `account` differs); both carry a `counterparty`
    /// pointing at the other side. A future refactor that drops one
    /// row would silently halve the recipient's balance history — this
    /// test is the only thing standing between that change and prod.
    #[test]
    fn transfer_emits_two_symmetric_rows_with_counterparty() {
        let rows = one(&[(
            3,
            DecodedBalanceEvent::Transfer {
                from: A,
                to: B,
                amount: 1_000,
            },
        )]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].account, A);
        assert_eq!(rows[0].kind, MovementKind::Transfer);
        assert_eq!(rows[0].delta, -1_000);
        assert_eq!(rows[0].counterparty, Some(B));
        assert_eq!(rows[1].account, B);
        assert_eq!(rows[1].kind, MovementKind::Transfer);
        assert_eq!(rows[1].delta, 1_000);
        assert_eq!(rows[1].counterparty, Some(A));
        // Symmetry: deltas sum to zero. Locks the "no value created"
        // invariant the Phase 5 reconciliation will later assume.
        assert_eq!(rows[0].delta + rows[1].delta, 0);
    }

    /// Single-row events: one fixture per tracked variant covering
    /// the full `MovementKind` enum minus `Transfer` (above). Locks
    /// the sign convention documented on `BalanceMovementRow::delta`.
    #[test]
    fn single_row_kinds_have_correct_signs_and_no_counterparty() {
        let cases: Vec<(DecodedBalanceEvent, MovementKind, i128)> = vec![
            (
                DecodedBalanceEvent::Deposit { who: A, amount: 10 },
                MovementKind::Deposit,
                10,
            ),
            (
                DecodedBalanceEvent::Withdraw { who: A, amount: 10 },
                MovementKind::Withdraw,
                -10,
            ),
            (
                DecodedBalanceEvent::Slash { who: A, amount: 10 },
                MovementKind::Slash,
                -10,
            ),
            (
                DecodedBalanceEvent::Reserve { who: A, amount: 10 },
                MovementKind::Reserve,
                10,
            ),
            (
                DecodedBalanceEvent::Unreserve { who: A, amount: 10 },
                MovementKind::Unreserve,
                -10,
            ),
            (
                DecodedBalanceEvent::Burn { who: A, amount: 10 },
                MovementKind::Burn,
                -10,
            ),
            (
                DecodedBalanceEvent::Mint { who: A, amount: 10 },
                MovementKind::Minted,
                10,
            ),
            (
                DecodedBalanceEvent::Frozen { who: A, amount: 10 },
                MovementKind::Frozen,
                10,
            ),
            (
                DecodedBalanceEvent::Thawed { who: A, amount: 10 },
                MovementKind::Thawed,
                -10,
            ),
            (
                DecodedBalanceEvent::Locked { who: A, amount: 10 },
                MovementKind::Locked,
                10,
            ),
            (
                DecodedBalanceEvent::Unlocked { who: A, amount: 10 },
                MovementKind::Unlocked,
                -10,
            ),
            (
                DecodedBalanceEvent::Fee { who: A, amount: 10 },
                MovementKind::Fee,
                -10,
            ),
            (
                DecodedBalanceEvent::Held { who: A, amount: 10 },
                MovementKind::Held,
                10,
            ),
            (
                DecodedBalanceEvent::Released { who: A, amount: 10 },
                MovementKind::Released,
                -10,
            ),
            (
                DecodedBalanceEvent::BurnedHeld { who: A, amount: 10 },
                MovementKind::BurnedHeld,
                -10,
            ),
            (
                DecodedBalanceEvent::Endowed {
                    account: A,
                    amount: 10,
                },
                MovementKind::Endowed,
                10,
            ),
            (
                DecodedBalanceEvent::DustLost {
                    account: A,
                    amount: 10,
                },
                MovementKind::DustLost,
                -10,
            ),
            (
                DecodedBalanceEvent::Restored { who: A, amount: 10 },
                MovementKind::Restored,
                10,
            ),
            (
                DecodedBalanceEvent::Suspended { who: A, amount: 10 },
                MovementKind::Suspended,
                -10,
            ),
        ];
        for (evt, expected_kind, expected_delta) in cases {
            let rows = one(&[(0, evt.clone())]);
            assert_eq!(rows.len(), 1, "event {evt:?} must emit a single row");
            assert_eq!(rows[0].kind, expected_kind);
            assert_eq!(rows[0].delta, expected_delta, "wrong sign for {evt:?}");
            assert!(
                rows[0].counterparty.is_none(),
                "counterparty must be None for non-Transfer (kind={expected_kind:?})"
            );
        }
    }

    /// `ReserveRepatriated` is the odd one out: it carries from/to on
    /// chain but the Phase 4 contract collapses it to a single row on
    /// the source side. Locks that explicit choice — a future change
    /// that switches to a 2-row shape needs to bump this test
    /// deliberately rather than slipping past CI.
    #[test]
    fn reserve_repatriated_emits_single_source_row_no_counterparty() {
        let rows = one(&[(
            7,
            DecodedBalanceEvent::ReserveRepatriated {
                from: A,
                amount: 250,
            },
        )]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].account, A);
        assert_eq!(rows[0].kind, MovementKind::ReserveRepatriated);
        assert_eq!(rows[0].delta, -250);
        assert!(rows[0].counterparty.is_none());
    }

    /// Discriminants are bound straight into a `SMALLINT` column; if
    /// the enum order drifts from the migration's comment, every
    /// historical row becomes a different `MovementKind` overnight.
    /// This test pins the i16 values so a reorder is caught at compile
    /// + test time rather than after the next backfill.
    #[test]
    fn movement_kind_smallint_round_trip() {
        let pairs: &[(MovementKind, i16)] = &[
            (MovementKind::Transfer, 0),
            (MovementKind::Deposit, 1),
            (MovementKind::Withdraw, 2),
            (MovementKind::Fee, 3),
            (MovementKind::Slash, 4),
            (MovementKind::Reserve, 5),
            (MovementKind::Unreserve, 6),
            (MovementKind::ReserveRepatriated, 7),
            (MovementKind::Burn, 8),
            (MovementKind::Minted, 9),
            (MovementKind::Frozen, 10),
            (MovementKind::Thawed, 11),
            (MovementKind::Locked, 12),
            (MovementKind::Unlocked, 13),
            (MovementKind::Held, 14),
            (MovementKind::Released, 15),
            (MovementKind::BurnedHeld, 16),
            (MovementKind::TransferAndHold, 17),
            (MovementKind::TransferOnHold, 18),
            (MovementKind::Endowed, 19),
            (MovementKind::DustLost, 20),
            (MovementKind::Restored, 21),
            (MovementKind::Suspended, 22),
        ];
        for (kind, want) in pairs {
            assert_eq!(kind.as_i16(), *want, "{kind:?} must serialise to {want}");
        }
    }

    /// `TransferAndHold` and `TransferOnHold` are two-row events like
    /// `Transfer` — source loses, dest gains, both carry the
    /// counterparty. Locks the symmetric shape so a future change that
    /// flattens one side into a single row is forced to update this
    /// test deliberately.
    #[test]
    fn transfer_and_hold_emits_two_symmetric_rows_with_counterparty() {
        for (decoded, kind) in [
            (
                DecodedBalanceEvent::TransferAndHold {
                    source: A,
                    dest: B,
                    amount: 1_000,
                },
                MovementKind::TransferAndHold,
            ),
            (
                DecodedBalanceEvent::TransferOnHold {
                    source: A,
                    dest: B,
                    amount: 1_000,
                },
                MovementKind::TransferOnHold,
            ),
        ] {
            let rows = one(&[(3, decoded)]);
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0].account, A);
            assert_eq!(rows[0].kind, kind);
            assert_eq!(rows[0].delta, -1_000);
            assert_eq!(rows[0].counterparty, Some(B));
            assert_eq!(rows[1].account, B);
            assert_eq!(rows[1].kind, kind);
            assert_eq!(rows[1].delta, 1_000);
            assert_eq!(rows[1].counterparty, Some(A));
            assert_eq!(rows[0].delta + rows[1].delta, 0);
        }
    }

    /// Multiple events from different accounts in the same block must
    /// preserve their event_idx attribution and produce stable
    /// ordering — the sink relies on idx + account being unique per
    /// block, so a stray duplication would trip the PK on insert.
    #[test]
    fn multiple_events_keep_event_idx_attribution() {
        let rows = one(&[
            (0, DecodedBalanceEvent::Deposit { who: A, amount: 5 }),
            (1, DecodedBalanceEvent::Withdraw { who: B, amount: 3 }),
            (
                2,
                DecodedBalanceEvent::Transfer {
                    from: A,
                    to: C,
                    amount: 7,
                },
            ),
        ]);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].event_idx, 0);
        assert_eq!(rows[1].event_idx, 1);
        assert_eq!(rows[2].event_idx, 2);
        assert_eq!(rows[3].event_idx, 2);
        // No two rows share the (event_idx, account) PK component.
        let mut keys: Vec<(u32, [u8; 32])> =
            rows.iter().map(|r| (r.event_idx, r.account)).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), rows.len(), "PK collision in projection output");
    }
}
