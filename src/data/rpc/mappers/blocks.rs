//! Block-level mapping: header → [`crate::domain::Block`], plus the storage
//! fetches (timestamp, weights, author) that populate the domain fields.

use subxt::client::OnlineClientAtBlock;
use subxt::config::substrate::{Digest, DigestItem, SubstrateHeader};
use subxt::ext::codec::{Decode, Encode};
use subxt::utils::{AccountId32, H256};
use subxt::SubstrateConfig;

use crate::data::error::{DataError, DataResult};
use crate::data::rpc::client::with_timeout;
use crate::data::rpc::runtime::allfeat;
use crate::data::ss58::{encode_ss58, short_label};
use crate::domain::Block;

use super::common::{fetch_block_events, hash_string};

/// Engine ID used by Aura `PreRuntime` digest items.
const AURA_ENGINE_ID: [u8; 4] = *b"aura";

/// Pluck the Aura slot from a header digest, if present and decodable.
pub fn aura_slot(digest: &Digest) -> Option<u64> {
    digest.logs.iter().find_map(|item| match item {
        DigestItem::PreRuntime(id, data) if *id == AURA_ENGINE_ID => {
            // PreRuntime payload for Aura is `sp_consensus_slots::Slot` which
            // is a transparent newtype around `u64`.
            u64::decode(&mut &data[..]).ok()
        }
        _ => None,
    })
}

/// Pick the block author from `(slot, validators)`. Returns `None` if the
/// validator set is empty (a degenerate dev-chain state we still render).
pub fn author_from_slot(slot: u64, validators: &[AccountId32]) -> Option<&AccountId32> {
    if validators.is_empty() {
        return None;
    }
    validators.get((slot % validators.len() as u64) as usize)
}

/// Pull the on-chain Block view for `at`. `finalized_head` lets the caller
/// tell us whether this block has been finalized without re-querying it for
/// every entry in a list.
pub async fn map_block(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    finalized_head: u64,
    ss58_prefix: u16,
) -> DataResult<Block> {
    let header = with_timeout("block_header", async {
        at.block_header()
            .await
            .map_err(|e| DataError::Rpc(format!("fetch header: {e}")))
    })
    .await?;

    let number = at.block_number();
    let hash = at.block_hash();

    let timestamp_ms = fetch_timestamp(at).await?;
    let (extrinsic_count, size_extrinsics) = fetch_extrinsics_summary(at).await?;
    let event_count = fetch_event_count(at).await?;
    let (author, author_name) = resolve_author(at, &header.digest, ss58_prefix).await?;
    let (ref_time, proof_size, ref_time_pct) = fetch_weights(at).await?;

    let size_bytes = (header.encoded_size() + size_extrinsics) as u32;

    Ok(Block {
        number,
        hash: hash_string(&hash),
        parent_hash: hash_string(&header.parent_hash),
        state_root: hash_string(&header.state_root),
        extrinsics_root: hash_string(&header.extrinsics_root),
        timestamp_ms,
        finalized: number <= finalized_head,
        extrinsic_count,
        event_count,
        author,
        author_name,
        ref_time,
        ref_time_pct,
        proof_size,
        spec_version: at.spec_version(),
        size_bytes,
    })
}

pub async fn fetch_timestamp(at: &OnlineClientAtBlock<SubstrateConfig>) -> DataResult<i64> {
    // `Timestamp::Now` is set by the inherent at the start of every block.
    // Genesis (#0) has no inherent yet, so the entry is missing — use 0.
    let value = with_timeout("fetch_timestamp", async {
        at.storage()
            .try_fetch(allfeat::storage().timestamp().now(), ())
            .await
            .map_err(|e| DataError::Rpc(format!("fetch timestamp: {e}")))
    })
    .await?;
    let Some(value) = value else {
        return Ok(0);
    };
    let now: u64 = value
        .decode()
        .map_err(|e| DataError::Decode(format!("decode timestamp: {e}")))?;
    Ok(now as i64)
}

pub async fn fetch_extrinsics_summary(
    at: &OnlineClientAtBlock<SubstrateConfig>,
) -> DataResult<(u32, usize)> {
    let extrinsics = with_timeout("fetch_extrinsics_summary", async {
        at.extrinsics()
            .fetch()
            .await
            .map_err(|e| DataError::Rpc(format!("fetch extrinsics: {e}")))
    })
    .await?;

    let mut count: u32 = 0;
    let mut bytes: usize = 0;
    for ext in extrinsics.iter() {
        let ext = ext.map_err(|e| DataError::Decode(format!("decode extrinsic: {e}")))?;
        bytes += ext.bytes().len();
        count += 1;
    }
    Ok((count, bytes))
}

async fn fetch_event_count(at: &OnlineClientAtBlock<SubstrateConfig>) -> DataResult<u32> {
    let events = fetch_block_events(at).await?;
    Ok(events.len())
}

async fn resolve_author(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    digest: &Digest,
    ss58_prefix: u16,
) -> DataResult<(String, String)> {
    let Some(slot) = aura_slot(digest) else {
        return Ok((String::from("unknown"), String::from("unknown")));
    };

    // `aura.authorities()` stores `sr25519::Public` session keys, not account
    // IDs — treating those bytes as an SS58 address yields unrelated (empty)
    // accounts. `session.validators()` is rebuilt in lockstep with the Aura
    // authority set (pallet-aura's `OneSessionHandler::on_new_session` maps
    // validators → session keys in order), so the same `slot % len` index
    // lands on the producing validator's actual account.
    let validators_value = with_timeout("fetch_session_validators", async {
        at.storage()
            .try_fetch(allfeat::storage().session().validators(), ())
            .await
            .map_err(|e| DataError::Rpc(format!("fetch session validators: {e}")))
    })
    .await?;
    let Some(validators_value) = validators_value else {
        return Ok((String::from("unknown"), String::from("unknown")));
    };

    let validators: Vec<AccountId32> = validators_value
        .decode()
        .map_err(|e| DataError::Decode(format!("decode session validators: {e}")))?;

    let Some(author_id) = author_from_slot(slot, &validators) else {
        return Ok((String::from("unknown"), String::from("unknown")));
    };

    let address = encode_ss58(&author_id.0, ss58_prefix);
    // No on-chain validator-name registry — surface a stable short label so the
    // UI's name slot is filled. The full SS58 sits next to it via `Addr`.
    let short_name = short_label(&address);
    Ok((address, short_name))
}

pub async fn fetch_weights(
    at: &OnlineClientAtBlock<SubstrateConfig>,
) -> DataResult<(u64, u64, u8)> {
    use allfeat::runtime_types::sp_weights::weight_v2::Weight;

    let consumed_value = with_timeout("fetch_block_weight", async {
        at.storage()
            .try_fetch(allfeat::storage().system().block_weight(), ())
            .await
            .map_err(|e| DataError::Rpc(format!("fetch block weight: {e}")))
    })
    .await?;
    let Some(consumed_value) = consumed_value else {
        return Ok((0, 0, 0));
    };

    let consumed: allfeat::runtime_types::frame_support::dispatch::PerDispatchClass<Weight> =
        consumed_value
            .decode()
            .map_err(|e| DataError::Decode(format!("decode block weight: {e}")))?;
    let total = sum_weights(&consumed);

    let max_block = at
        .constants()
        .entry(allfeat::constants().system().block_weights())
        .map(|bws| bws.max_block.ref_time)
        .unwrap_or(0);
    let pct = total
        .ref_time
        .saturating_mul(100)
        .checked_div(max_block)
        .map(|p| p.min(100) as u8)
        .unwrap_or(0);

    Ok((total.ref_time, total.proof_size, pct))
}

fn sum_weights(
    consumed: &allfeat::runtime_types::frame_support::dispatch::PerDispatchClass<
        allfeat::runtime_types::sp_weights::weight_v2::Weight,
    >,
) -> allfeat::runtime_types::sp_weights::weight_v2::Weight {
    allfeat::runtime_types::sp_weights::weight_v2::Weight {
        ref_time: consumed
            .normal
            .ref_time
            .saturating_add(consumed.operational.ref_time)
            .saturating_add(consumed.mandatory.ref_time),
        proof_size: consumed
            .normal
            .proof_size
            .saturating_add(consumed.operational.proof_size)
            .saturating_add(consumed.mandatory.proof_size),
    }
}

#[allow(dead_code)]
fn _digest_type_check(h: &SubstrateHeader<H256>) -> &Digest {
    // Compile-time guard: keep the mapper aligned with `SubstrateConfig`'s
    // header shape. If the type changes upstream the rest of the file stops
    // compiling here first.
    &h.digest
}

#[cfg(test)]
mod tests {
    use super::*;
    use subxt::ext::codec::Encode;

    #[test]
    fn aura_slot_decodes_pre_runtime_digest() {
        let slot: u64 = 0x1234_5678_9abc_def0;
        let digest = Digest {
            logs: vec![
                DigestItem::Other(b"noise".to_vec()),
                DigestItem::PreRuntime(AURA_ENGINE_ID, slot.encode()),
                DigestItem::Seal(AURA_ENGINE_ID, vec![0xff; 64]),
            ],
        };
        assert_eq!(aura_slot(&digest), Some(slot));
    }

    #[test]
    fn aura_slot_skips_other_engines_and_returns_none_when_absent() {
        let digest = Digest {
            logs: vec![
                DigestItem::PreRuntime(*b"BABE", 7u64.encode()),
                DigestItem::Consensus(AURA_ENGINE_ID, 9u64.encode()),
            ],
        };
        assert_eq!(aura_slot(&digest), None);
    }

    #[test]
    fn author_from_slot_wraps_around_authority_set() {
        let authorities: Vec<AccountId32> =
            (0..3).map(|i| AccountId32::from([i as u8; 32])).collect();

        assert_eq!(
            author_from_slot(0, &authorities).map(AccountId32::as_ref),
            Some([0u8; 32].as_ref()),
        );
        assert_eq!(
            author_from_slot(7, &authorities).map(AccountId32::as_ref),
            Some([1u8; 32].as_ref()),
        );
    }

    #[test]
    fn author_from_slot_returns_none_on_empty_set() {
        let authorities: Vec<AccountId32> = Vec::new();
        assert!(author_from_slot(42, &authorities).is_none());
    }
}
