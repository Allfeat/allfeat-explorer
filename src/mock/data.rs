//! Deterministic chain-data generator.
//!
//! Every getter is parameterized by a [`ChainCtx`] (network spec + wall-clock
//! `now_ms`) so two networks at the same input never collide and the head
//! ticks forward in real time on the client.

use super::rng::{hash_str, Lcg};
use crate::domain::{
    claimable_amount, Account, Allocation, AtsFeedItem, AtsRecord, AtsStats, AtsVersion, Balance,
    Block, CallField, CallResult, Deposit, EnvelopeDetail, EnvelopeId, EnvelopeInfo, EpochInfo,
    EventRef, Extrinsic, ExtrinsicArgs, TokenOverview, Transfer, TreasuryInfo, VERSION_DEPOSIT,
};
use crate::network::ChainCtx;

const HEX_ALPHA: &[u8] = b"abcdef0123456789";
const SS58_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// Lowercase hex-alphanumeric sequence of `len` chars, seeded for determinism.
fn hex_chars_from_seed(len: usize, seed: u32) -> String {
    let mut rng = Lcg::new(seed);
    let mut out = String::with_capacity(len + 2);
    out.push_str("0x");
    for _ in 0..len {
        let v = rng.next_u32();
        out.push(HEX_ALPHA[(v % HEX_ALPHA.len() as u32) as usize] as char);
    }
    out
}

const MODULES: &[(&str, &[&str])] = &[
    (
        "balances",
        &[
            "transfer_keep_alive",
            "transfer_all",
            "transfer_allow_death",
            "force_transfer",
        ],
    ),
    ("timestamp", &["set"]),
    ("system", &["remark", "remark_with_event", "set_code"]),
    ("utility", &["batch", "batch_all", "force_batch"]),
    ("session", &["set_keys", "purge_keys"]),
    ("proxy", &["add_proxy", "remove_proxy", "proxy"]),
    ("democracy", &["vote", "propose"]),
    ("parachainSystem", &["set_validation_data"]),
];

const EVENTS: &[&str] = &[
    "balances.Transfer",
    "balances.Deposit",
    "balances.Withdraw",
    "system.ExtrinsicSuccess",
    "system.NewAccount",
    "treasury.Deposit",
    "session.NewSession",
    "utility.BatchCompleted",
];

#[inline]
fn mix(ctx: ChainCtx, x: u32) -> u32 {
    ctx.spec.seed.wrapping_add(x)
}

pub fn hex_seeded(len: usize, seed: u32) -> String {
    hex_chars_from_seed(len, seed)
}

pub fn ss58_seeded(seed: u32) -> String {
    let mut rng = Lcg::new(seed);
    let mut out = String::with_capacity(48);
    out.push('5');
    for _ in 0..47 {
        let v = rng.next_u32();
        out.push(SS58_ALPHABET[(v as usize) % SS58_ALPHABET.len()] as char);
    }
    out
}

// Role seeds for the mocked "known accounts". Each is mixed with the
// network seed via `mix(...)`, so every network produces a distinct SS58
// per role while the role assignment itself is stable.
//
// The frontend re-derives the exact same SS58 via a TS port of
// `ss58_seeded` + these constants (see web/app/utils/mockSs58.ts and
// web/app/utils/knownAccounts.ts). When touching the formula below,
// update the TS side in lockstep or the registry labels will stop
// matching.
pub const SEED_SUDO: u32 = 0x5000_0001;
pub const SEED_TREASURY: u32 = 0x5000_0002;
pub const VALIDATOR_POOL: u32 = 8;
pub const SEED_VALIDATOR_BASE: u32 = 0x5000_0100;

fn seed_validator(i: u32) -> u32 {
    SEED_VALIDATOR_BASE + (i % VALIDATOR_POOL)
}

pub fn mock_sudo(ctx: ChainCtx) -> String {
    ss58_seeded(mix(ctx, SEED_SUDO))
}

pub fn mock_treasury(ctx: ChainCtx) -> String {
    ss58_seeded(mix(ctx, SEED_TREASURY))
}

pub fn mock_validator(ctx: ChainCtx, i: u32) -> String {
    ss58_seeded(mix(ctx, seed_validator(i)))
}

pub fn build_block(ctx: ChainCtx, num: u64) -> Block {
    let head = ctx.head_block();
    let delta = head.saturating_sub(num);
    let ts = ctx.block_timestamp_ms(num);
    let seed = mix(ctx, num.wrapping_mul(31).wrapping_add(7) as u32);
    let mut rng = Lcg::new(seed);

    let extrinsic_count = 2 + rng.gen_range(4);
    let event_count = 40 + rng.gen_range(40);
    let ref_time_base = 150_000_000u64;
    let ref_time = ref_time_base + rng.gen_range(600_000_000) as u64;
    let ref_time_pct = (rng.next_f32() * 40.0 + 5.0).round() as u8;
    let proof_size = rng.gen_range(800_000) as u64 + 100_000;
    let size_bytes = 1800 + rng.gen_range(18_000);

    // Cycle over the reserved validator pool so the known-accounts
    // registry (frontend) can label every block author. `author_name`
    // stays in lockstep: both keys off `num % VALIDATOR_POOL` and
    // `authors.len()` happens to match (8).
    let validator_idx = (num as u32) % VALIDATOR_POOL;
    let author = mock_validator(ctx, validator_idx);
    let authors = ctx.spec.authors;
    let author_name = authors[(num as usize) % authors.len()].to_string();

    Block {
        number: num,
        hash: hex_seeded(64, mix(ctx, (num.wrapping_mul(13).wrapping_add(1)) as u32)),
        parent_hash: hex_seeded(
            64,
            mix(
                ctx,
                (num.saturating_sub(1).wrapping_mul(13).wrapping_add(1)) as u32,
            ),
        ),
        state_root: hex_seeded(64, mix(ctx, (num.wrapping_mul(17).wrapping_add(3)) as u32)),
        extrinsics_root: hex_seeded(64, mix(ctx, (num.wrapping_mul(19).wrapping_add(5)) as u32)),
        timestamp_ms: ts,
        finalized: delta >= 2,
        extrinsic_count,
        event_count,
        author,
        author_name,
        ref_time,
        ref_time_pct,
        proof_size,
        spec_version: ctx.spec.spec_version,
        size_bytes,
    }
}

pub fn build_extrinsics(ctx: ChainCtx, block_number: u64, count: u32) -> Vec<Extrinsic> {
    let ts = ctx.block_timestamp_ms(block_number);
    let mut out = Vec::with_capacity(count as usize);

    out.push(Extrinsic {
        id: format!("{block_number}-0"),
        block_number,
        index: 0,
        hash: hex_seeded(64, mix(ctx, (block_number.wrapping_mul(113)) as u32)),
        module: "timestamp".into(),
        call: "set".into(),
        signed: false,
        signer: None,
        args: ExtrinsicArgs::Decoded {
            fields: vec![CallField {
                name: Some("now".into()),
                type_name: Some("Compact<T::Moment>".into()),
                value: ts.to_string(),
            }],
        },
        result: CallResult::Success,
        nonce: None,
        tip: 0,
        fee: 0,
        timestamp_ms: ts,
        events: vec![EventRef {
            module: "system".into(),
            name: "ExtrinsicSuccess".into(),
            fields: Vec::new(),
        }],
    });

    let mut rng = Lcg::new(mix(
        ctx,
        (block_number.wrapping_mul(37).wrapping_add(11)) as u32,
    ));
    let non_timestamp: Vec<(&str, &[&str])> = MODULES
        .iter()
        .copied()
        .filter(|(name, _)| *name != "timestamp")
        .collect();

    for i in 1..count {
        let (module, calls) = non_timestamp[rng.gen_range(non_timestamp.len() as u32) as usize];
        let call = calls[rng.gen_range(calls.len() as u32) as usize];
        let signer_seed = mix(ctx, rng.gen_range(9999) + block_number as u32 + i);
        // Every 11th extrinsic of each block is signed by the sudo key —
        // guarantees sudo shows up as an occasional extrinsic signer
        // without monopolising the feed.
        let signer = if (block_number as u32 + i) % 11 == 3 {
            mock_sudo(ctx)
        } else {
            ss58_seeded(signer_seed)
        };
        let amount_raw = rng.gen_range(1_000_000) as u128 * 1_000_000;
        let success = rng.next_f32() > 0.07;

        let args = match module {
            "balances" => {
                // Route ~1 in 5 balance transfers to the treasury so the
                // Latest-transfers / block-transfer feeds regularly show
                // the "Treasury" label in action.
                let dest = if (block_number as u32 + i) % 5 == 2 {
                    mock_treasury(ctx)
                } else {
                    ss58_seeded(mix(ctx, block_number as u32 + i + 42))
                };
                mock_balances_transfer_fields(dest, amount_raw)
            }
            _ => ExtrinsicArgs::Raw {
                hex: "0x".to_owned() + &"ab".repeat(8),
            },
        };

        let nonce = rng.gen_range(200);
        let fee = (rng.gen_range(2_000_000_000) + 15_000_000) as u128;
        let event_n = 3 + rng.gen_range(5);
        let events: Vec<EventRef> = (0..event_n)
            .map(|_| {
                let e = EVENTS[rng.gen_range(EVENTS.len() as u32) as usize];
                let (m, n) = e.split_once('.').unwrap_or((e, ""));
                EventRef {
                    module: m.to_string(),
                    name: n.to_string(),
                    fields: Vec::new(),
                }
            })
            .collect();

        out.push(Extrinsic {
            id: format!("{block_number}-{i}"),
            block_number,
            index: i,
            hash: hex_seeded(
                64,
                mix(ctx, (block_number.wrapping_mul(113) + i as u64) as u32),
            ),
            module: module.to_string(),
            call: call.to_string(),
            signed: true,
            signer: Some(signer),
            args,
            result: if success {
                CallResult::Success
            } else {
                CallResult::Failed
            },
            nonce: Some(nonce),
            tip: 0,
            fee,
            timestamp_ms: ts,
            events,
        });
    }
    out
}

pub fn get_blocks(ctx: ChainCtx, count: u32, from_num: u64) -> Vec<Block> {
    (0..count as u64)
        .filter_map(|i| {
            let num = from_num.checked_sub(i)?;
            Some(build_block(ctx, num))
        })
        .collect()
}

pub fn get_latest_extrinsics(ctx: ChainCtx, count: usize) -> Vec<Extrinsic> {
    let mut out = Vec::with_capacity(count);
    let mut b = ctx.head_block();
    while out.len() < count && b > 0 {
        let blk = build_block(ctx, b);
        let ex: Vec<Extrinsic> = build_extrinsics(ctx, b, blk.extrinsic_count)
            .into_iter()
            .skip(1)
            .collect();
        for e in ex {
            if out.len() < count {
                out.push(e);
            }
        }
        b -= 1;
    }
    out
}

pub fn get_transfers(ctx: ChainCtx, count: usize) -> Vec<Transfer> {
    let mut out = Vec::with_capacity(count);
    let head = ctx.head_block();
    let mut b = head;
    let floor = head.saturating_sub(200);
    while out.len() < count && b > floor {
        let blk = build_block(ctx, b);
        let extrinsics = build_extrinsics(ctx, b, blk.extrinsic_count + 2);
        for e in extrinsics.into_iter().filter(|e| e.module == "balances") {
            if out.len() >= count {
                break;
            }
            if let (Some(signer), Some((dest, amount))) =
                (e.signer.clone(), balances_transfer_args(&e.args))
            {
                out.push(Transfer {
                    extrinsic: e,
                    from: signer,
                    to: dest,
                    amount,
                });
            }
        }
        b -= 1;
    }
    out
}

/// Build the `Balances.transfer*` argument shape the UI reads for
/// transfer rows — matches the field names the live runtime metadata
/// emits (`dest`, `value`) so the fixture path and the indexed path
/// render identically.
fn mock_balances_transfer_fields(dest: String, value: u128) -> ExtrinsicArgs {
    ExtrinsicArgs::Decoded {
        fields: vec![
            CallField {
                name: Some("dest".into()),
                type_name: Some("AccountIdLookupOf<T>".into()),
                value: dest,
            },
            CallField {
                name: Some("value".into()),
                type_name: Some("Compact<T::Balance>".into()),
                value: value.to_string(),
            },
        ],
    }
}

/// Pull `(dest, value)` out of a `Balances.transfer*` args shape the
/// mock just built. Parses `value` as a decimal u128 (what
/// [`mock_balances_transfer_fields`] emits); anything else surfaces as
/// `None` so the transfer feed skips the row instead of inventing one.
pub fn balances_transfer_args(args: &ExtrinsicArgs) -> Option<(String, u128)> {
    let ExtrinsicArgs::Decoded { fields } = args else {
        return None;
    };
    let dest = fields
        .iter()
        .find(|f| f.name.as_deref() == Some("dest"))?
        .value
        .clone();
    let value = fields
        .iter()
        .find(|f| f.name.as_deref() == Some("value"))?
        .value
        .parse::<u128>()
        .ok()?;
    Some((dest, value))
}

pub fn get_account(ctx: ChainCtx, addr: &str) -> Account {
    let mut rng = Lcg::new(hash_str(addr) ^ ctx.spec.seed);
    let total = rng.gen_range(50_000) as u128 * 1_000_000_000_000;
    let reserved = (total as f32 * (rng.next_f32() * 0.3)) as u128;
    let transferable = total.saturating_sub(reserved);
    let nonce = rng.gen_range(400);
    let first_seen_ms = ctx.now_ms - (rng.gen_range(300) as i64) * 86_400_000;
    let last_active_ms = ctx.now_ms - (rng.gen_range(3600) as i64) * 1_000;

    Account {
        address: addr.to_string(),
        balance: Balance {
            total,
            transferable,
            reserved,
        },
        nonce,
        first_seen_ms,
        last_active_ms,
    }
}

pub fn get_top_accounts(ctx: ChainCtx, count: usize) -> Vec<Account> {
    // Seed the pool with the known-role accounts so the Top Accounts
    // page always surfaces treasury / sudo / validators with their
    // labels. The rest of the slots are filled with the usual
    // deterministic-random synthetic accounts.
    let mut v: Vec<Account> = Vec::with_capacity(count);
    v.push(get_account(ctx, &mock_treasury(ctx)));
    v.push(get_account(ctx, &mock_sudo(ctx)));
    for i in 0..VALIDATOR_POOL {
        if v.len() >= count {
            break;
        }
        v.push(get_account(ctx, &mock_validator(ctx, i)));
    }
    let mut i: u32 = 0;
    while v.len() < count {
        let addr = ss58_seeded(mix(ctx, i.wrapping_mul(131).wrapping_add(9)));
        v.push(get_account(ctx, &addr));
        i += 1;
    }
    v.sort_by_key(|a| std::cmp::Reverse(a.balance.total));
    v
}

/* ==== ATS ====================================================== */

pub fn get_ats_stats(ctx: ChainCtx) -> AtsStats {
    let total = ctx.ats_total();
    let total_versions = ctx.ats_version_total();
    let bt = ctx.spec.block_time_secs.max(1);
    // Counters scale with what would actually fit per period at the chain's cadence.
    let per_day = 86_400 / bt;
    let last_24h = (per_day / ctx.spec.ats_blocks_per.max(1) as u64) as u32;
    let last_7d = last_24h.saturating_mul(7);
    let last_30d = last_24h.saturating_mul(30);
    AtsStats {
        total,
        total_versions,
        last_24h,
        last_7d,
        last_30d,
        unique_owners: (total as f32 * 0.42).round() as u32,
        avg_per_day: last_24h,
        protocol_version: 1,
        genesis_block: 0,
        first_registered_at_ms: ctx.spec.genesis_ms,
        total_deposited: total_versions as u128 * VERSION_DEPOSIT,
        multi_version_share: 0.40,
    }
}

fn ats_id_from_index(ctx: ChainCtx, index: u32) -> u32 {
    ctx.ats_total().saturating_sub(index).max(1)
}

pub fn build_ats(ctx: ChainCtx, index: u32) -> AtsRecord {
    let total = ctx.ats_total();
    let ats_id = ats_id_from_index(ctx, index);
    let i = total - ats_id; // 0 = newest
    let seed = mix(
        ctx,
        (ats_id as u64).wrapping_mul(2_654_435_761).wrapping_add(7) as u32,
    );
    let mut rng = Lcg::new(seed);

    let head_block = ctx.head_block().saturating_sub(3);
    let genesis = head_block.saturating_sub(220_000);
    let span = head_block.saturating_sub(genesis);
    let offset = (i as u64 * span) / total.max(1) as u64 + rng.gen_range(20) as u64;
    let created_at_block = head_block.saturating_sub(offset);
    let created_at_ms = ctx.block_timestamp_ms(created_at_block);

    let owner = ss58_seeded(mix(ctx, 1000 + ats_id * 37));

    let v_roll = rng.next_f32();
    let version_count = if v_roll < 0.60 {
        1
    } else if v_roll < 0.85 {
        2
    } else if v_roll < 0.95 {
        3
    } else if v_roll < 0.99 {
        4 + rng.gen_range(2)
    } else {
        5 + rng.gen_range(2)
    };

    let mut deposits = vec![Deposit {
        address: owner.clone(),
        amount: VERSION_DEPOSIT,
    }];
    if version_count > 2 && rng.next_f32() > 0.6 {
        deposits.push(Deposit {
            address: ss58_seeded(mix(ctx, 2000 + ats_id * 13)),
            amount: VERSION_DEPOSIT,
        });
    }
    if version_count > 4 && rng.next_f32() > 0.5 {
        deposits.push(Deposit {
            address: ss58_seeded(mix(ctx, 3000 + ats_id * 11)),
            amount: VERSION_DEPOSIT,
        });
    }
    let total_deposit = deposits.iter().map(|d| d.amount).sum();

    let mut versions = Vec::with_capacity(version_count as usize);
    let mut v_block = created_at_block;
    for v in 0..version_count {
        let v_seed = mix(
            ctx,
            ats_id
                .wrapping_mul(1_000_003)
                .wrapping_add(v.wrapping_mul(17)),
        );
        let mut vr = Lcg::new(v_seed);
        if v > 0 {
            v_block += 300 + vr.gen_range(12_000) as u64;
            if v_block > head_block {
                v_block = head_block.saturating_sub(vr.gen_range(100) as u64);
            }
        }
        let v_ts = ctx.block_timestamp_ms(v_block);
        let commitment = hex_seeded(
            64,
            mix(
                ctx,
                ats_id
                    .wrapping_mul(991)
                    .wrapping_add(v * 131)
                    .wrapping_add(13),
            ),
        );
        let extrinsic_index = 1 + vr.gen_range(6);
        let fee = vr.gen_range(800_000_000) as u128 + 120_000_000;
        let signer = if v == 0 {
            owner.clone()
        } else if v % 2 == 0 {
            deposits
                .get(1)
                .map(|d| d.address.clone())
                .unwrap_or_else(|| owner.clone())
        } else {
            owner.clone()
        };
        versions.push(AtsVersion {
            version_index: v,
            commitment,
            protocol_version: 1,
            created_at_ms: v_ts,
            block_number: v_block,
            extrinsic_index,
            extrinsic_id: format!("{v_block}-{extrinsic_index}"),
            fee,
            signer,
        });
    }

    AtsRecord {
        ats_id,
        owner,
        created_at_ms,
        created_at_block,
        version_count,
        deposits,
        total_deposit,
        versions,
    }
}

pub fn get_ats_list(ctx: ChainCtx, count: u32, from_index: u32) -> Vec<AtsRecord> {
    (0..count)
        .filter_map(|k| {
            let idx = from_index.checked_add(k)?;
            let record = build_ats(ctx, idx);
            (record.ats_id >= 1).then_some(record)
        })
        .collect()
}

pub fn get_ats_version_feed(ctx: ChainCtx, count: u32, from_index: u32) -> Vec<AtsFeedItem> {
    let total = ctx.ats_total();
    let mut out: Vec<AtsFeedItem> = Vec::with_capacity(count as usize);
    let mut ats_index: u32 = 0;
    let mut skipped: u32 = 0;
    while out.len() < count as usize && ats_index < total {
        let a = build_ats(ctx, ats_index);
        for v in a.versions.iter().rev() {
            if skipped < from_index {
                skipped += 1;
                continue;
            }
            if out.len() >= count as usize {
                break;
            }
            out.push(AtsFeedItem {
                ats_id: a.ats_id,
                owner: a.owner.clone(),
                version_index: v.version_index,
                is_initial: v.version_index == 0,
                is_latest: v.version_index == a.version_count - 1,
                version_count: a.version_count,
                commitment: v.commitment.clone(),
                protocol_version: v.protocol_version,
                block_number: v.block_number,
                extrinsic_id: v.extrinsic_id.clone(),
                timestamp_ms: v.created_at_ms,
                signer: v.signer.clone(),
            });
        }
        ats_index += 1;
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.timestamp_ms));
    out.truncate(count as usize);
    out
}

pub fn get_account_ats_count(ctx: ChainCtx, addr: &str) -> u32 {
    let mut rng = Lcg::new(hash_str(addr) ^ 0x41_53_54_41 ^ ctx.spec.seed);
    let roll = rng.next_f32();
    if roll < 0.55 {
        0
    } else if roll < 0.85 {
        1 + rng.gen_range(3)
    } else if roll < 0.98 {
        4 + rng.gen_range(12)
    } else {
        20 + rng.gen_range(80)
    }
}

pub fn get_account_ats(ctx: ChainCtx, addr: &str, limit: u32) -> Vec<AtsRecord> {
    let n = get_account_ats_count(ctx, addr);
    if n == 0 {
        return Vec::new();
    }
    let mut rng = Lcg::new(hash_str(addr) ^ ctx.spec.seed);
    let cap = n.min(limit);
    let total = ctx.ats_total();
    let mut out = Vec::with_capacity(cap as usize);
    for _ in 0..cap {
        let idx = rng.gen_range(total.min(4000));
        let mut record = build_ats(ctx, idx);
        record.owner = addr.to_string();
        if !record.deposits.is_empty() {
            record.deposits[0] = Deposit {
                address: addr.to_string(),
                amount: VERSION_DEPOSIT,
            };
        }
        out.push(record);
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.created_at_ms));
    out
}

/* ==== Token allocation (mainnet-only pallet) ===================== */

// Mainnet has 12 decimals; total nominal supply is 1B AFT.
const AFT_DECIMALS: u8 = 12;
const ONE_AFT: u128 = 1_000_000_000_000;
const TOTAL_SUPPLY_AFT: u128 = 1_000_000_000;
const TOTAL_SUPPLY_PLANCK: u128 = TOTAL_SUPPLY_AFT * ONE_AFT;
// 1 month = 30 days; at 6s per block on Allfeat mainnet that's 432_000 blocks.
const BLOCKS_PER_MONTH: u64 = 432_000;
// Epoch granularity for the on-chain scheduler — every ~3.5 days the loop
// pays out the eligible vesting deltas.
const EPOCH_DURATION_BLOCKS: u64 = 50_000;

/// Per-envelope tokenomics: cap fraction (basis points of total supply),
/// upfront % paid at allocation, cliff (months), vesting duration (months),
/// and whether the cap goes to a single auto-allocated beneficiary.
struct EnvelopeSpec {
    id: EnvelopeId,
    cap_bps: u32,
    upfront_pct: u8,
    cliff_months: u64,
    vesting_months: u64,
    /// Number of allocations to synthesize for the mock. Ignored when the
    /// envelope has a unique beneficiary (always 1).
    allocation_count: u32,
    unique: bool,
}

const ENVELOPE_SPECS: [EnvelopeSpec; 13] = [
    // Teams: long cliff, large vest. The ~5-month-old mainnet sits ~1mo into
    // the vest, so the table shows partial released amounts.
    EnvelopeSpec {
        id: EnvelopeId::Teams,
        cap_bps: 1_500,
        upfront_pct: 0,
        cliff_months: 4,
        vesting_months: 24,
        allocation_count: 18,
        unique: false,
    },
    EnvelopeSpec {
        id: EnvelopeId::KoL,
        cap_bps: 150,
        upfront_pct: 10,
        cliff_months: 3,
        vesting_months: 12,
        allocation_count: 14,
        unique: false,
    },
    EnvelopeSpec {
        id: EnvelopeId::Private1,
        cap_bps: 500,
        upfront_pct: 15,
        cliff_months: 1,
        vesting_months: 12,
        allocation_count: 22,
        unique: false,
    },
    EnvelopeSpec {
        id: EnvelopeId::Private2,
        cap_bps: 500,
        upfront_pct: 15,
        cliff_months: 1,
        vesting_months: 12,
        allocation_count: 28,
        unique: false,
    },
    EnvelopeSpec {
        id: EnvelopeId::Public1,
        cap_bps: 100,
        upfront_pct: 25,
        cliff_months: 0,
        vesting_months: 6,
        allocation_count: 36,
        unique: false,
    },
    EnvelopeSpec {
        id: EnvelopeId::Public2,
        cap_bps: 100,
        upfront_pct: 25,
        cliff_months: 0,
        vesting_months: 6,
        allocation_count: 42,
        unique: false,
    },
    EnvelopeSpec {
        id: EnvelopeId::Public3,
        cap_bps: 150,
        upfront_pct: 30,
        cliff_months: 0,
        vesting_months: 4,
        allocation_count: 48,
        unique: false,
    },
    EnvelopeSpec {
        id: EnvelopeId::Public4,
        cap_bps: 150,
        upfront_pct: 30,
        cliff_months: 0,
        vesting_months: 4,
        allocation_count: 54,
        unique: false,
    },
    // Airdrop: 100% upfront, no vest — table shows allocations but with all
    // amounts already released.
    EnvelopeSpec {
        id: EnvelopeId::Airdrop,
        cap_bps: 400,
        upfront_pct: 100,
        cliff_months: 0,
        vesting_months: 0,
        allocation_count: 60,
        unique: false,
    },
    EnvelopeSpec {
        id: EnvelopeId::CommunityRewards,
        cap_bps: 2_500,
        upfront_pct: 0,
        cliff_months: 0,
        vesting_months: 36,
        allocation_count: 32,
        unique: false,
    },
    // Listing: single beneficiary, fully unlocked at genesis (liquidity).
    EnvelopeSpec {
        id: EnvelopeId::Listing,
        cap_bps: 500,
        upfront_pct: 100,
        cliff_months: 0,
        vesting_months: 0,
        allocation_count: 1,
        unique: true,
    },
    EnvelopeSpec {
        id: EnvelopeId::ResearchDevelopment,
        cap_bps: 1_200,
        upfront_pct: 0,
        cliff_months: 6,
        vesting_months: 24,
        allocation_count: 8,
        unique: false,
    },
    // Reserve: single beneficiary, long cliff. Currently still in cliff so
    // its sub-account holds the full cap.
    EnvelopeSpec {
        id: EnvelopeId::Reserve,
        cap_bps: 2_250,
        upfront_pct: 0,
        cliff_months: 6,
        vesting_months: 24,
        allocation_count: 1,
        unique: true,
    },
];

/// Mock pallet sub-account: distinct seed per envelope so addresses don't
/// collide with the existing treasury / sudo / validator pool.
const SEED_TOKEN_ENVELOPE_BASE: u32 = 0x5000_0300;

fn envelope_account(ctx: ChainCtx, id: EnvelopeId) -> String {
    ss58_seeded(mix(
        ctx,
        SEED_TOKEN_ENVELOPE_BASE + id.variant_index() as u32,
    ))
}

/// Build one allocation given its envelope spec and a deterministic per-id
/// seed. The released amount is derived from the chain head so the mock
/// surfaces realistic "% vested" progression without needing time-travel.
#[allow(clippy::too_many_arguments)]
fn build_allocation(
    ctx: ChainCtx,
    head: u64,
    spec: &EnvelopeSpec,
    alloc_id: u32,
    beneficiary: String,
    total: u128,
    cliff_blocks: u64,
    vesting_blocks: u64,
) -> Allocation {
    let upfront = total.saturating_mul(spec.upfront_pct as u128) / 100;
    let vested_total = total.saturating_sub(upfront);
    let start_block = cliff_blocks;

    // Released so far: anything fully past its vesting_duration is fully
    // released; mid-vest entries land at a deterministic fraction of what's
    // been claimable so the table shows a non-trivial "released" delta.
    let released = if vested_total == 0 {
        0
    } else if head >= start_block.saturating_add(vesting_blocks) {
        vested_total
    } else if head <= start_block {
        0
    } else {
        let mut rng = Lcg::new(mix(ctx, SEED_TOKEN_ENVELOPE_BASE + 1_000 + alloc_id));
        let claimable = claimable_amount(head, start_block, vested_total, 0, vesting_blocks);
        // Release between 70% and 100% of the currently claimable amount —
        // simulates beneficiaries who poll the epoch payout.
        let factor = 0.70 + rng.next_f32() * 0.30;
        ((claimable as f64) * factor as f64) as u128
    };

    let claimable_now = claimable_amount(head, start_block, vested_total, released, vesting_blocks);
    let percent_vested = released
        .saturating_mul(100)
        .checked_div(vested_total)
        .map_or(100u8, |v| v.min(100) as u8);

    Allocation {
        id: alloc_id,
        envelope: spec.id,
        beneficiary,
        total,
        upfront,
        vested_total,
        released,
        start_block,
        claimable_now,
        percent_vested,
    }
}

fn build_envelope_allocations(ctx: ChainCtx, head: u64, spec: &EnvelopeSpec) -> Vec<Allocation> {
    let cap = TOTAL_SUPPLY_PLANCK * spec.cap_bps as u128 / 10_000;
    let cliff_blocks = spec.cliff_months * BLOCKS_PER_MONTH;
    let vesting_blocks = spec.vesting_months * BLOCKS_PER_MONTH;

    if spec.unique {
        // Whole cap goes to one beneficiary, allocated at genesis. Mirrors
        // the pallet's auto unique-beneficiary path.
        let beneficiary = ss58_seeded(mix(
            ctx,
            SEED_TOKEN_ENVELOPE_BASE + 500 + spec.id.variant_index() as u32,
        ));
        let alloc_id = (spec.id.variant_index() as u32) * 100;
        return vec![build_allocation(
            ctx,
            head,
            spec,
            alloc_id,
            beneficiary,
            cap,
            cliff_blocks,
            vesting_blocks,
        )];
    }

    // Distribution model: ~80% of the cap is allocated across N beneficiaries
    // with a long-tail split (a couple of large rounds + many smaller ones),
    // leaving 20% in the envelope sub-account as undistributed reserve. The
    // sum is normalised exactly to the target amount so the totals match.
    let target_distributed = cap * 80 / 100;
    let n = spec.allocation_count.max(1) as usize;
    let base_seed = mix(
        ctx,
        SEED_TOKEN_ENVELOPE_BASE + 200 + (spec.id.variant_index() as u32) * 31,
    );

    // Generate raw weights, then scale to the target.
    let mut raw_weights: Vec<u64> = Vec::with_capacity(n);
    let mut sum_weight: u64 = 0;
    let mut rng = Lcg::new(base_seed);
    for _ in 0..n {
        // Long-tailed weights: 1..=1_000.
        let w = (rng.gen_range(900) + 100) as u64;
        raw_weights.push(w);
        sum_weight += w;
    }

    let mut out = Vec::with_capacity(n);
    let mut allocated: u128 = 0;
    for (i, w) in raw_weights.iter().enumerate() {
        // Last allocation absorbs any rounding residual so the sum lands
        // exactly on `target_distributed`.
        let amount = if i + 1 == n {
            target_distributed.saturating_sub(allocated)
        } else {
            let share = target_distributed.saturating_mul(*w as u128) / sum_weight as u128;
            allocated = allocated.saturating_add(share);
            share
        };

        let alloc_id = (spec.id.variant_index() as u32) * 100 + i as u32 + 1;
        let beneficiary = ss58_seeded(mix(ctx, base_seed.wrapping_add(i as u32 * 17 + 1)));
        out.push(build_allocation(
            ctx,
            head,
            spec,
            alloc_id,
            beneficiary,
            amount,
            cliff_blocks,
            vesting_blocks,
        ));
    }
    out
}

fn envelope_info(ctx: ChainCtx, spec: &EnvelopeSpec, allocations: &[Allocation]) -> EnvelopeInfo {
    let cap = TOTAL_SUPPLY_PLANCK * spec.cap_bps as u128 / 10_000;
    let distributed: u128 = allocations.iter().map(|a| a.total).sum();
    let cliff_blocks = spec.cliff_months * BLOCKS_PER_MONTH;
    let vesting_blocks = spec.vesting_months * BLOCKS_PER_MONTH;
    let unique_beneficiary = if spec.unique {
        allocations.first().map(|a| a.beneficiary.clone())
    } else {
        None
    };

    EnvelopeInfo {
        id: spec.id,
        label: spec.id.label().to_string(),
        account: envelope_account(ctx, spec.id),
        total_cap: cap,
        distributed,
        upfront_pct: spec.upfront_pct,
        cliff_blocks,
        vesting_duration_blocks: vesting_blocks,
        unique_beneficiary,
        allocation_count: allocations.len() as u32,
    }
}

fn spec_for(id: EnvelopeId) -> &'static EnvelopeSpec {
    ENVELOPE_SPECS
        .iter()
        .find(|s| s.id == id)
        .expect("EnvelopeId is exhaustive")
}

pub fn get_envelope_detail(ctx: ChainCtx, id: EnvelopeId) -> EnvelopeDetail {
    let head = ctx.head_block();
    let spec = spec_for(id);
    let mut allocations = build_envelope_allocations(ctx, head, spec);
    // Largest-first — matches what users care about on a per-envelope page.
    allocations.sort_by_key(|a| std::cmp::Reverse(a.total));
    let envelope = envelope_info(ctx, spec, &allocations);
    EnvelopeDetail {
        envelope,
        allocations,
    }
}

pub fn get_token_overview(ctx: ChainCtx) -> TokenOverview {
    let head = ctx.head_block();
    let mut envelopes = Vec::with_capacity(EnvelopeId::ALL.len());
    let mut locked: u128 = 0;
    let mut envelope_reserves: u128 = 0;
    let treasury_addr = mock_treasury(ctx);
    let mut treasury_locked: u128 = 0;

    for spec in &ENVELOPE_SPECS {
        let cap = TOTAL_SUPPLY_PLANCK * spec.cap_bps as u128 / 10_000;
        let allocations = build_envelope_allocations(ctx, head, spec);
        // Locked = sum of (vested_total - released) across all allocations
        // (i.e. amounts still held under the vesting hold reason).
        for a in &allocations {
            let held = a.vested_total.saturating_sub(a.released);
            locked = locked.saturating_add(held);
            if a.beneficiary == treasury_addr {
                treasury_locked = treasury_locked.saturating_add(held);
            }
        }
        let info = envelope_info(ctx, spec, &allocations);
        envelope_reserves = envelope_reserves.saturating_add(cap.saturating_sub(info.distributed));
        envelopes.push(info);
    }

    let circulating = TOTAL_SUPPLY_PLANCK
        .saturating_sub(locked)
        .saturating_sub(envelope_reserves);

    let treasury = TreasuryInfo {
        balance: get_account(ctx, &treasury_addr).balance.total,
        account: treasury_addr,
        locked: treasury_locked,
    };

    let next_payout_block = ((head / EPOCH_DURATION_BLOCKS) + 1) * EPOCH_DURATION_BLOCKS;
    let epoch = EpochInfo {
        index: head / EPOCH_DURATION_BLOCKS,
        head_block: head,
        next_payout_block,
        epoch_duration_blocks: EPOCH_DURATION_BLOCKS,
    };

    TokenOverview {
        symbol: ctx.spec.token.to_string(),
        decimals: AFT_DECIMALS,
        total_supply: TOTAL_SUPPLY_PLANCK,
        circulating,
        locked,
        envelope_reserves,
        treasury,
        epoch,
        envelopes,
    }
}

pub fn get_account_allocations(ctx: ChainCtx, address: &str) -> Vec<Allocation> {
    let head = ctx.head_block();
    let mut out = Vec::new();
    for spec in &ENVELOPE_SPECS {
        for a in build_envelope_allocations(ctx, head, spec) {
            if a.beneficiary == address {
                out.push(a);
            }
        }
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.total));
    out
}
