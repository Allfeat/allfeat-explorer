//! `pallet-token-allocation` mapping: envelopes, allocations, epoch state,
//! treasury. Mainnet-only pallet — callers must gate on the network id
//! before invoking any of these helpers.
//!
//! Unlike the other mappers in this directory, this one doesn't dispatch on
//! [`crate::network::RuntimeKind`]: `pallet-token-allocation` and
//! `pallet-treasury` are absent from the Melodie runtime, so every public
//! entry point is guarded upstream by
//! [`crate::data::provider::ensure_token_network`] (which returns
//! [`DataError::NotSupported`](crate::data::error::DataError::NotSupported) on
//! anything other than `allfeat`). Adding a Melodie arm here would dead-code
//! the dispatch and tempt future readers into wiring up calls that the
//! provider would reject anyway. If a Melodie-side allocation pallet ever
//! lands, route it through a sibling module rather than collapsing the two
//! shapes into a fake `match`.
//!
//! The pallet exposes three storage items the explorer cares about:
//! * `Envelopes(EnvelopeId) -> EnvelopeConfig` — static tokenomics per budget
//!   pocket (cap, upfront %, cliff, vesting duration).
//! * `EnvelopeDistributed(EnvelopeId) -> u128` — running counter of how much
//!   of the cap has been handed out.
//! * `Allocations(u32) -> Allocation` — per-beneficiary entries with their
//!   live vesting state (total / upfront / vested / released).
//!
//! The mappers iterate `Allocations` once (bucketed by envelope) and pair the
//! results with `Envelopes` / `EnvelopeDistributed` reads — the full pallet
//! footprint for a single overview request is ~30 storage keys today.

use subxt::client::OnlineClientAtBlock;
use subxt::utils::AccountId32;
use subxt::SubstrateConfig;

use crate::data::error::{DataError, DataResult};
use crate::data::rpc::client::{with_iter_timeout, with_timeout};
use crate::data::rpc::runtime::allfeat;
use crate::data::ss58::encode_ss58;
use crate::domain::{
    claimable_amount, Allocation, EnvelopeDetail, EnvelopeId, EnvelopeInfo, EpochInfo,
    TokenOverview, TreasuryInfo,
};

use super::accounts::parse_ss58;

/// Token decimals for the Allfeat `AFT` token. Substrate exposes this via
/// `system_properties` (a JSON-RPC call, not a runtime constant), so we mirror
/// the value the mock reports rather than pulling it over the wire on every
/// overview request. If a future runtime repurposes the token, update this
/// constant alongside `NetworkSpec::token`.
const AFT_DECIMALS: u8 = 12;

type OnChainEnvelopeId = allfeat::runtime_types::pallet_token_allocation::EnvelopeId;
type OnChainEnvelopeConfig =
    allfeat::runtime_types::pallet_token_allocation::EnvelopeConfig<u128, u32, AccountId32>;
type OnChainAllocation =
    allfeat::runtime_types::pallet_token_allocation::Allocation<AccountId32, u128, u32>;

/// Domain → on-chain `EnvelopeId`. The two enums share variant names and order
/// but live in different modules — the compiler can't infer the mapping.
fn to_onchain_envelope_id(id: EnvelopeId) -> OnChainEnvelopeId {
    match id {
        EnvelopeId::Teams => OnChainEnvelopeId::Teams,
        EnvelopeId::KoL => OnChainEnvelopeId::KoL,
        EnvelopeId::Private1 => OnChainEnvelopeId::Private1,
        EnvelopeId::Private2 => OnChainEnvelopeId::Private2,
        EnvelopeId::Public1 => OnChainEnvelopeId::Public1,
        EnvelopeId::Public2 => OnChainEnvelopeId::Public2,
        EnvelopeId::Public3 => OnChainEnvelopeId::Public3,
        EnvelopeId::Public4 => OnChainEnvelopeId::Public4,
        EnvelopeId::Airdrop => OnChainEnvelopeId::Airdrop,
        EnvelopeId::CommunityRewards => OnChainEnvelopeId::CommunityRewards,
        EnvelopeId::Listing => OnChainEnvelopeId::Listing,
        EnvelopeId::ResearchDevelopment => OnChainEnvelopeId::ResearchDevelopment,
        EnvelopeId::Reserve => OnChainEnvelopeId::Reserve,
    }
}

fn to_domain_envelope_id(id: &OnChainEnvelopeId) -> EnvelopeId {
    match id {
        OnChainEnvelopeId::Teams => EnvelopeId::Teams,
        OnChainEnvelopeId::KoL => EnvelopeId::KoL,
        OnChainEnvelopeId::Private1 => EnvelopeId::Private1,
        OnChainEnvelopeId::Private2 => EnvelopeId::Private2,
        OnChainEnvelopeId::Public1 => EnvelopeId::Public1,
        OnChainEnvelopeId::Public2 => EnvelopeId::Public2,
        OnChainEnvelopeId::Public3 => EnvelopeId::Public3,
        OnChainEnvelopeId::Public4 => EnvelopeId::Public4,
        OnChainEnvelopeId::Airdrop => EnvelopeId::Airdrop,
        OnChainEnvelopeId::CommunityRewards => EnvelopeId::CommunityRewards,
        OnChainEnvelopeId::Listing => EnvelopeId::Listing,
        OnChainEnvelopeId::ResearchDevelopment => EnvelopeId::ResearchDevelopment,
        OnChainEnvelopeId::Reserve => EnvelopeId::Reserve,
    }
}

/// Reconstruct a pallet sub-account via `frame_support::PalletId::
/// into_sub_account_truncating(envelope_id)` without pulling the frame crates
/// into the explorer: payload is `"modl" ++ pallet_id ++ SCALE(enum) ++ 0…`,
/// truncated/padded to 32 bytes. The enum SCALE encoding is a single variant
/// byte, so 4 + 8 + 1 = 13 bytes of payload and 19 zero bytes of padding.
fn envelope_sub_account(pallet_id: &[u8; 8], id: EnvelopeId) -> AccountId32 {
    let mut buf = [0u8; 32];
    buf[..4].copy_from_slice(b"modl");
    buf[4..12].copy_from_slice(pallet_id);
    buf[12] = id.variant_index();
    AccountId32::from(buf)
}

/// Read `TokenAllocation::PalletId` constant (8 bytes). The value is baked
/// into the runtime metadata, so a decode failure here signals a genuine
/// runtime mismatch and we surface it instead of defaulting.
fn fetch_token_pallet_id(at: &OnlineClientAtBlock<SubstrateConfig>) -> DataResult<[u8; 8]> {
    let value = at
        .constants()
        .entry(allfeat::constants().token_allocation().pallet_id())
        .map_err(|e| DataError::Decode(format!("decode TokenAllocation::PalletId: {e}")))?;
    Ok(value.0)
}

/// Read `Treasury::pot_account` — a convenience constant the runtime emits
/// alongside `PalletId` so clients don't have to redo the sub-account
/// derivation for the treasury pot.
fn fetch_treasury_pot_account(
    at: &OnlineClientAtBlock<SubstrateConfig>,
) -> DataResult<AccountId32> {
    at.constants()
        .entry(allfeat::constants().treasury().pot_account())
        .map_err(|e| DataError::Decode(format!("decode Treasury::pot_account: {e}")))
}

/// `Balances::TotalIssuance`. ValueQuery default is `0`.
async fn fetch_total_issuance(at: &OnlineClientAtBlock<SubstrateConfig>) -> DataResult<u128> {
    let value = with_timeout("fetch_total_issuance", async {
        at.storage()
            .try_fetch(allfeat::storage().balances().total_issuance(), ())
            .await
            .map_err(|e| DataError::Rpc(format!("fetch balances.total_issuance: {e}")))
    })
    .await?;
    let Some(value) = value else {
        return Ok(0);
    };
    value
        .decode()
        .map_err(|e| DataError::Decode(format!("decode TotalIssuance: {e}")))
}

/// Free + reserved balance for an arbitrary account id. Returns `(0, 0)` for
/// reaped accounts so the caller can present the treasury / envelope pots with
/// a zero balance rather than a hard error.
async fn fetch_account_balance(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    id: AccountId32,
) -> DataResult<(u128, u128)> {
    let value = with_timeout("fetch_account_balance", async {
        at.storage()
            .try_fetch(allfeat::storage().system().account(), (id,))
            .await
            .map_err(|e| DataError::Rpc(format!("fetch system.account: {e}")))
    })
    .await?;
    let Some(value) = value else {
        return Ok((0, 0));
    };
    let info = value
        .decode()
        .map_err(|e| DataError::Decode(format!("decode AccountInfo: {e}")))?;
    Ok((info.data.free, info.data.reserved))
}

/// `TokenAllocation::Envelopes(id)`. `None` when the pallet hasn't registered
/// a config for this envelope yet (bootstrap window before genesis migration
/// lands).
async fn fetch_envelope_config(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    id: EnvelopeId,
) -> DataResult<Option<OnChainEnvelopeConfig>> {
    let key = to_onchain_envelope_id(id);
    let value = with_timeout("fetch_envelope_config", async {
        at.storage()
            .try_fetch(allfeat::storage().token_allocation().envelopes(), (key,))
            .await
            .map_err(|e| DataError::Rpc(format!("fetch token_allocation.envelopes: {e}")))
    })
    .await?;
    let Some(value) = value else {
        return Ok(None);
    };
    value
        .decode()
        .map(Some)
        .map_err(|e| DataError::Decode(format!("decode EnvelopeConfig: {e}")))
}

/// `TokenAllocation::EnvelopeDistributed(id)`. ValueQuery default is `0` so a
/// missing entry surfaces as zero-distributed rather than an error.
async fn fetch_envelope_distributed(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    id: EnvelopeId,
) -> DataResult<u128> {
    let key = to_onchain_envelope_id(id);
    let value = with_timeout("fetch_envelope_distributed", async {
        at.storage()
            .try_fetch(
                allfeat::storage().token_allocation().envelope_distributed(),
                (key,),
            )
            .await
            .map_err(|e| {
                DataError::Rpc(format!("fetch token_allocation.envelope_distributed: {e}"))
            })
    })
    .await?;
    let Some(value) = value else {
        return Ok(0);
    };
    value
        .decode()
        .map_err(|e| DataError::Decode(format!("decode EnvelopeDistributed: {e}")))
}

async fn fetch_epoch_index(at: &OnlineClientAtBlock<SubstrateConfig>) -> DataResult<u64> {
    let value = with_timeout("fetch_epoch_index", async {
        at.storage()
            .try_fetch(allfeat::storage().token_allocation().epoch_index(), ())
            .await
            .map_err(|e| DataError::Rpc(format!("fetch token_allocation.epoch_index: {e}")))
    })
    .await?;
    let Some(value) = value else {
        return Ok(0);
    };
    value
        .decode()
        .map_err(|e| DataError::Decode(format!("decode EpochIndex: {e}")))
}

async fn fetch_next_payout_at(at: &OnlineClientAtBlock<SubstrateConfig>) -> DataResult<u32> {
    let value = with_timeout("fetch_next_payout_at", async {
        at.storage()
            .try_fetch(allfeat::storage().token_allocation().next_payout_at(), ())
            .await
            .map_err(|e| DataError::Rpc(format!("fetch token_allocation.next_payout_at: {e}")))
    })
    .await?;
    let Some(value) = value else {
        return Ok(0);
    };
    value
        .decode()
        .map_err(|e| DataError::Decode(format!("decode NextPayoutAt: {e}")))
}

fn fetch_epoch_duration(at: &OnlineClientAtBlock<SubstrateConfig>) -> DataResult<u32> {
    at.constants()
        .entry(allfeat::constants().token_allocation().epoch_duration())
        .map_err(|e| DataError::Decode(format!("decode TokenAllocation::EpochDuration: {e}")))
}

/// Walk `TokenAllocation::Allocations`, pairing each entry with its allocation
/// id. The pallet never revokes entries (pure history + vesting bookkeeping),
/// so this returns the full live set. Today's mainnet carries on the order of
/// a few hundred allocations — a full scan is fine and keeps the aggregation
/// logic simple. Revisit with an indexer projection if the count grows past
/// ~10k.
async fn iter_allocations(
    at: &OnlineClientAtBlock<SubstrateConfig>,
) -> DataResult<Vec<(u32, OnChainAllocation)>> {
    with_iter_timeout("iter_allocations", async {
        let mut entries = at
            .storage()
            .iter(allfeat::storage().token_allocation().allocations(), ())
            .await
            .map_err(|e| DataError::Rpc(format!("iter token_allocation.allocations: {e}")))?;

        let mut out = Vec::new();
        while let Some(kv) = entries.next().await {
            let kv = kv.map_err(|e| {
                DataError::Rpc(format!("iter token_allocation.allocations next: {e}"))
            })?;
            let (id,) = kv
                .key()
                .map_err(|e| DataError::Decode(format!("allocations key: {e}")))?
                .decode()
                .map_err(|e| DataError::Decode(format!("decode allocations key: {e}")))?;
            let alloc = kv
                .value()
                .decode()
                .map_err(|e| DataError::Decode(format!("decode Allocation: {e}")))?;
            out.push((id, alloc));
        }
        Ok(out)
    })
    .await
}

/// Turn an on-chain allocation + its envelope config into the domain shape.
/// `vesting_duration` is looked up from the config so the explorer can
/// recompute `claimable_now` without a second round-trip.
fn build_allocation(
    alloc_id: u32,
    alloc: &OnChainAllocation,
    vesting_duration: u32,
    head_block: u64,
    ss58_prefix: u16,
) -> Allocation {
    let start_block = alloc.start as u64;
    let vesting_duration = vesting_duration as u64;
    let claimable_now = claimable_amount(
        head_block,
        start_block,
        alloc.vested_total,
        alloc.released,
        vesting_duration,
    );
    let percent_vested = alloc
        .released
        .saturating_mul(100)
        .checked_div(alloc.vested_total)
        .map(|pct| pct.min(100) as u8)
        // `vested_total == 0` means the allocation was fully upfront (or
        // uninitialised) — treat that as "nothing left to vest".
        .unwrap_or(100);
    Allocation {
        id: alloc_id,
        envelope: to_domain_envelope_id(&alloc.envelope),
        beneficiary: encode_ss58(&alloc.beneficiary.0, ss58_prefix),
        total: alloc.total,
        upfront: alloc.upfront,
        vested_total: alloc.vested_total,
        released: alloc.released,
        start_block,
        claimable_now,
        percent_vested,
    }
}

/// Build the per-envelope summary. `distributed` + `allocation_count` are
/// aggregated from the caller's side (so a single allocations scan feeds every
/// envelope on the overview) instead of re-iterating here.
fn build_envelope_info(
    pallet_id: &[u8; 8],
    id: EnvelopeId,
    config: &OnChainEnvelopeConfig,
    distributed: u128,
    unique_beneficiary: Option<String>,
    allocation_count: u32,
    ss58_prefix: u16,
) -> EnvelopeInfo {
    let sub = envelope_sub_account(pallet_id, id);
    EnvelopeInfo {
        id,
        label: id.label().to_string(),
        account: encode_ss58(&sub.0, ss58_prefix),
        total_cap: config.total_cap,
        distributed,
        upfront_pct: config.upfront_rate.0,
        cliff_blocks: config.cliff as u64,
        vesting_duration_blocks: config.vesting_duration as u64,
        unique_beneficiary,
        allocation_count,
    }
}

/// Build the global `/token/overview` payload. Iterates `Allocations` once and
/// bucketizes by envelope so per-envelope counts, locked balances, and unique
/// beneficiary resolution all come from a single storage scan.
pub async fn build_token_overview(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    head_block: u64,
    token_symbol: &str,
    ss58_prefix: u16,
) -> DataResult<TokenOverview> {
    let pallet_id = fetch_token_pallet_id(at)?;
    let epoch_duration = fetch_epoch_duration(at)?;
    let treasury_account = fetch_treasury_pot_account(at)?;

    let (total_supply, epoch_index, next_payout_at) = futures::try_join!(
        fetch_total_issuance(at),
        fetch_epoch_index(at),
        fetch_next_payout_at(at),
    )?;

    let (treasury_free, _reserved) = fetch_account_balance(at, treasury_account).await?;

    // Bucketize allocations by envelope so each envelope summary only walks
    // the slice it cares about. Also tally `locked` across the whole set,
    // and the subset held specifically for the treasury account.
    let allocations = iter_allocations(at).await?;
    let mut locked: u128 = 0;
    let mut treasury_locked: u128 = 0;
    let mut buckets: std::collections::HashMap<EnvelopeId, Vec<(u32, OnChainAllocation)>> =
        std::collections::HashMap::with_capacity(EnvelopeId::ALL.len());
    for (id, alloc) in allocations {
        let held = alloc.vested_total.saturating_sub(alloc.released);
        locked = locked.saturating_add(held);
        if alloc.beneficiary == treasury_account {
            treasury_locked = treasury_locked.saturating_add(held);
        }
        let envelope = to_domain_envelope_id(&alloc.envelope);
        buckets.entry(envelope).or_default().push((id, alloc));
    }

    let treasury = TreasuryInfo {
        account: encode_ss58(&treasury_account.0, ss58_prefix),
        balance: treasury_free,
        locked: treasury_locked,
    };

    // Fan out the per-envelope reads (2 storage keys × 13 envelopes) so the
    // overview cold-path is one round-trip batch instead of 26 serial awaits.
    let envelope_reads =
        futures::future::try_join_all(EnvelopeId::ALL.iter().map(|&id| async move {
            let (config, distributed) = futures::try_join!(
                fetch_envelope_config(at, id),
                fetch_envelope_distributed(at, id),
            )?;
            DataResult::Ok((id, config, distributed))
        }))
        .await?;

    let mut envelopes = Vec::with_capacity(EnvelopeId::ALL.len());
    let mut envelope_reserves: u128 = 0;
    for (id, config, distributed) in envelope_reads {
        let Some(config) = config else {
            // Envelope not registered on-chain yet — skip it rather than
            // surfacing a zeroed entry that would dilute the overview.
            continue;
        };
        let bucket = buckets.remove(&id).unwrap_or_default();
        let unique_beneficiary = config
            .unique_beneficiary
            .as_ref()
            .map(|acct| encode_ss58(&acct.0, ss58_prefix));
        let info = build_envelope_info(
            &pallet_id,
            id,
            &config,
            distributed,
            unique_beneficiary,
            bucket.len() as u32,
            ss58_prefix,
        );
        envelope_reserves =
            envelope_reserves.saturating_add(config.total_cap.saturating_sub(distributed));
        envelopes.push(info);
    }

    let circulating = total_supply
        .saturating_sub(locked)
        .saturating_sub(envelope_reserves);

    let next_payout_block = next_payout_at as u64;
    let epoch = EpochInfo {
        index: epoch_index,
        head_block,
        next_payout_block,
        epoch_duration_blocks: epoch_duration as u64,
    };

    Ok(TokenOverview {
        symbol: token_symbol.to_string(),
        decimals: AFT_DECIMALS,
        total_supply,
        circulating,
        locked,
        envelope_reserves,
        treasury,
        epoch,
        envelopes,
    })
}

/// Per-envelope detail for `/token/envelopes/:id`. Returns `None` when the
/// envelope isn't registered on-chain — surfaces as a 404 at the API boundary.
pub async fn build_envelope_detail(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    id: EnvelopeId,
    head_block: u64,
    ss58_prefix: u16,
) -> DataResult<Option<EnvelopeDetail>> {
    let Some(config) = fetch_envelope_config(at, id).await? else {
        return Ok(None);
    };
    let pallet_id = fetch_token_pallet_id(at)?;
    let distributed = fetch_envelope_distributed(at, id).await?;

    // Walk the full allocations map and filter by envelope id. At today's
    // scale (< 1k allocations per mainnet envelope) this is a handful of
    // milliseconds; if a future deployment blows past that, swap in a
    // per-envelope secondary index.
    let all = iter_allocations(at).await?;
    let mut allocations: Vec<Allocation> = all
        .into_iter()
        .filter(|(_, a)| to_domain_envelope_id(&a.envelope) == id)
        .map(|(aid, alloc)| {
            build_allocation(
                aid,
                &alloc,
                config.vesting_duration,
                head_block,
                ss58_prefix,
            )
        })
        .collect();

    // Largest-first — matches the per-envelope UI expectation and mirrors the
    // mock generator's ordering.
    allocations.sort_by_key(|a| std::cmp::Reverse(a.total));

    let unique_beneficiary = config
        .unique_beneficiary
        .as_ref()
        .map(|acct| encode_ss58(&acct.0, ss58_prefix));
    let envelope = build_envelope_info(
        &pallet_id,
        id,
        &config,
        distributed,
        unique_beneficiary,
        allocations.len() as u32,
        ss58_prefix,
    );

    Ok(Some(EnvelopeDetail {
        envelope,
        allocations,
    }))
}

/// All allocations held by a single account. Malformed SS58 resolves to an
/// empty list so the account page shows "no allocations" instead of erroring.
pub async fn build_account_allocations(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    address: &str,
    head_block: u64,
    ss58_prefix: u16,
) -> DataResult<Vec<Allocation>> {
    let Some(target) = parse_ss58(address) else {
        return Ok(Vec::new());
    };
    let all = iter_allocations(at).await?;
    let target_bytes: &[u8; 32] = target.as_ref();

    // Cache per-envelope configs for vesting_duration lookup. A given account
    // rarely spans every envelope, so populate on demand rather than upfront.
    let mut config_cache: std::collections::HashMap<EnvelopeId, u32> =
        std::collections::HashMap::new();

    let mut out = Vec::new();
    for (aid, alloc) in all {
        let alloc_bytes: &[u8; 32] = alloc.beneficiary.as_ref();
        if alloc_bytes != target_bytes {
            continue;
        }
        let envelope = to_domain_envelope_id(&alloc.envelope);
        let vesting_duration = if let Some(v) = config_cache.get(&envelope) {
            *v
        } else {
            let v = fetch_envelope_config(at, envelope)
                .await?
                .map(|cfg| cfg.vesting_duration)
                .unwrap_or(0);
            config_cache.insert(envelope, v);
            v
        };
        out.push(build_allocation(
            aid,
            &alloc,
            vesting_duration,
            head_block,
            ss58_prefix,
        ));
    }

    out.sort_by_key(|a| std::cmp::Reverse(a.total));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_id_roundtrips_through_onchain_type() {
        for id in EnvelopeId::ALL {
            let onchain = to_onchain_envelope_id(id);
            let back = to_domain_envelope_id(&onchain);
            assert_eq!(id, back, "envelope id roundtrip failed for {id:?}");
        }
    }

    #[test]
    fn envelope_variant_indices_match_codegen_indices() {
        // Mirrors the `#[codec(index = N)]` annotations in the generated
        // runtime_types — if the pallet renumbers a variant this guard trips
        // first instead of silently corrupting sub-account derivation.
        assert_eq!(EnvelopeId::Teams.variant_index(), 0);
        assert_eq!(EnvelopeId::KoL.variant_index(), 1);
        assert_eq!(EnvelopeId::Reserve.variant_index(), 12);
    }

    #[test]
    fn envelope_sub_account_matches_substrate_derivation() {
        // "m/tkenalc" = pallet id for TokenAllocation (ASCII bytes from the
        // runtime metadata, verified via `subxt explore`).
        let pallet_id: [u8; 8] = [109, 47, 116, 107, 110, 97, 108, 99];
        let acct = envelope_sub_account(&pallet_id, EnvelopeId::Teams);
        let bytes: &[u8; 32] = acct.as_ref();
        assert_eq!(&bytes[..4], b"modl", "must start with frame prefix");
        assert_eq!(&bytes[4..12], &pallet_id, "pallet id in place");
        assert_eq!(bytes[12], 0, "Teams = variant 0");
        assert!(bytes[13..].iter().all(|b| *b == 0), "trailing zero padding");
    }

    #[test]
    fn envelope_sub_accounts_differ_per_envelope() {
        let pallet_id: [u8; 8] = [109, 47, 116, 107, 110, 97, 108, 99];
        let a = envelope_sub_account(&pallet_id, EnvelopeId::Teams);
        let b = envelope_sub_account(&pallet_id, EnvelopeId::Reserve);
        assert_ne!(
            a, b,
            "different envelopes must hash to different sub-accounts"
        );
    }

    #[test]
    fn claimable_amount_pre_cliff_is_zero() {
        assert_eq!(claimable_amount(100, 200, 1_000, 0, 500), 0);
        assert_eq!(claimable_amount(200, 200, 1_000, 0, 500), 0);
    }

    #[test]
    fn claimable_amount_post_vest_returns_unreleased_remainder() {
        // head well past start + duration → full vested_total minus released.
        assert_eq!(claimable_amount(10_000, 100, 1_000, 300, 500), 700);
    }

    #[test]
    fn claimable_amount_zero_duration_unlocks_full_remainder() {
        // No vesting window: everything is claimable immediately once start
        // is reached.
        assert_eq!(claimable_amount(200, 100, 1_000, 200, 0), 800);
    }

    #[test]
    fn claimable_amount_mid_vest_prorates_linearly() {
        // Halfway through vesting with nothing released yet → half the total.
        assert_eq!(claimable_amount(600, 100, 1_000, 0, 1_000), 500);
        // Same window but 200 already released → 500 - 200 = 300 left.
        assert_eq!(claimable_amount(600, 100, 1_000, 200, 1_000), 300);
    }

    fn fixture_alloc(total: u128, vested: u128, released: u128, start: u32) -> OnChainAllocation {
        OnChainAllocation {
            envelope: OnChainEnvelopeId::Teams,
            beneficiary: AccountId32::from([5u8; 32]),
            total,
            upfront: total.saturating_sub(vested),
            vested_total: vested,
            released,
            start,
        }
    }

    #[test]
    fn build_allocation_computes_percent_vested_and_claimable() {
        let alloc = fixture_alloc(1_000, 800, 200, 100);
        let domain = build_allocation(42, &alloc, 1_000, 600, 42);
        assert_eq!(domain.id, 42);
        assert_eq!(domain.envelope, EnvelopeId::Teams);
        assert_eq!(domain.total, 1_000);
        assert_eq!(domain.vested_total, 800);
        assert_eq!(domain.released, 200);
        assert_eq!(domain.start_block, 100);
        // head=600, start=100, elapsed=500, duration=1000 → 800 * 500 / 1000 = 400
        // claimable = 400 - 200 = 200.
        assert_eq!(domain.claimable_now, 200);
        // percent_vested = released * 100 / vested_total = 200 * 100 / 800 = 25.
        assert_eq!(domain.percent_vested, 25);
    }

    #[test]
    fn build_allocation_with_zero_vested_total_reports_full_vested() {
        let alloc = fixture_alloc(500, 0, 0, 0);
        let domain = build_allocation(7, &alloc, 0, 0, 42);
        assert_eq!(domain.percent_vested, 100);
        assert_eq!(domain.claimable_now, 0);
    }
}
