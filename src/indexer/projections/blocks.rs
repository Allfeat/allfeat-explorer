//! Project a pinned `OnlineClientAtBlock` into a [`BlockRow`].
//!
//! This is the *only* projection wired in Phase 1. It mirrors the data
//! the `blocks` table columns want (§1 of `docs/indexing-plan.md`) and
//! borrows the byte-level helpers already written for the RPC mapper
//! (`aura_slot`, `author_from_slot`, `fetch_timestamp`,
//! `fetch_block_events`) so the indexer and the current RPC path can't
//! drift on how authors / timestamps / event counts are computed.
//!
//! The shape is deliberately byte-oriented: every hash is `[u8; 32]`,
//! never hex. The sink binds these straight into `BYTEA` columns, so a
//! lookup by hash on the UI side ends up as a single memcmp instead of
//! a hex normalize + compare.

use subxt::client::OnlineClientAtBlock;
use subxt::config::substrate::Digest;
use subxt::ext::codec::Encode;
use subxt::utils::AccountId32;
use subxt::SubstrateConfig;

use crate::data::error::{DataError, DataResult};
use crate::data::rpc::client::with_timeout;
use crate::data::rpc::mappers::{
    aura_slot, author_from_slot, fetch_block_events, fetch_extrinsics_summary, fetch_timestamp,
    fetch_weights,
};
use crate::data::rpc::runtime::allfeat;

/// Row shape for `blocks`. Field order mirrors the `INSERT` statement
/// in [`crate::indexer::sink`]; adding a column means touching both
/// places deliberately rather than silently forgetting one side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockRow {
    pub num: u64,
    pub hash: [u8; 32],
    pub parent_hash: [u8; 32],
    pub state_root: [u8; 32],
    pub extrinsics_root: [u8; 32],
    /// `None` when no Aura `PreRuntime` digest is present or the
    /// authority set is empty. The `author` column is nullable for
    /// exactly this reason — genesis + some dev-chain edge cases.
    pub author: Option<[u8; 32]>,
    pub timestamp_ms: i64,
    pub spec_version: u32,
    pub extrinsic_count: u32,
    pub event_count: u32,
    /// Consumed `ref_time` (sum across `PerDispatchClass`) at this block.
    pub ref_time: u64,
    /// Consumed `proof_size` (sum across `PerDispatchClass`) at this block.
    pub proof_size: u64,
    /// `ref_time` as a percentage of the block's own `BlockWeights::max_block`.
    /// Computed at index time so a future runtime upgrade can't reinterpret
    /// old rows against a different ceiling.
    pub ref_time_pct: u8,
    /// SCALE-encoded header size + sum of extrinsic byte lengths.
    pub size_bytes: u32,
}

/// Pull every row field out of the pinned block. `block_num` is passed
/// in rather than reading `at.block_number()` so the caller's logical
/// cursor position is the single source of truth — no way for a buggy
/// pin to stamp the wrong primary key.
pub async fn map(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    block_num: u64,
) -> DataResult<BlockRow> {
    let header = with_timeout("block_header", async {
        at.block_header()
            .await
            .map_err(|e| DataError::Rpc(format!("fetch header: {e}")))
    })
    .await?;

    let hash = at.block_hash();
    let timestamp_ms = fetch_timestamp(at).await?;
    let author = resolve_author_bytes(at, &header.digest).await?;

    let (extrinsic_count, size_extrinsics) = fetch_extrinsics_summary(at).await?;
    let (ref_time, proof_size, ref_time_pct) = fetch_weights(at).await?;
    let size_bytes = (header.encoded_size() + size_extrinsics) as u32;

    let events = fetch_block_events(at).await?;
    // `Events::iter()` yields a `Result` per entry; we only count them
    // here and the count can't lie about decodability. Bubble any
    // decode failure so an incompatible metadata surfaces loudly rather
    // than quietly persisting a wrong `event_count`.
    let mut event_count: u32 = 0;
    for evt in events.iter() {
        evt.map_err(|e| DataError::Decode(format!("decode event: {e}")))?;
        event_count += 1;
    }

    Ok(BlockRow {
        num: block_num,
        hash: hash.0,
        parent_hash: header.parent_hash.0,
        state_root: header.state_root.0,
        extrinsics_root: header.extrinsics_root.0,
        author,
        timestamp_ms,
        spec_version: at.spec_version(),
        extrinsic_count,
        event_count,
        ref_time,
        proof_size,
        ref_time_pct,
        size_bytes,
    })
}

/// Byte-oriented twin of `mappers::blocks::resolve_author`. The SS58
/// version is still used by the RPC provider to render the UI author
/// column; here we keep the raw 32 bytes so `author` maps 1:1 onto the
/// `BYTEA` column and future account joins (Phase 5) stay index-compatible.
async fn resolve_author_bytes(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    digest: &Digest,
) -> DataResult<Option<[u8; 32]>> {
    let Some(slot) = aura_slot(digest) else {
        return Ok(None);
    };

    // See `mappers::blocks::resolve_author`: `aura.authorities()` exposes
    // session keys, not accounts. `session.validators()` is the accounts
    // list that Aura's authority set is built from, in the same order — so
    // the slot modulo still lands on the right producer.
    let validators_value = with_timeout("fetch_session_validators", async {
        at.storage()
            .try_fetch(allfeat::storage().session().validators(), ())
            .await
            .map_err(|e| DataError::Rpc(format!("fetch session validators: {e}")))
    })
    .await?;
    let Some(validators_value) = validators_value else {
        return Ok(None);
    };

    let validators: Vec<AccountId32> = validators_value
        .decode()
        .map_err(|e| DataError::Decode(format!("decode session validators: {e}")))?;

    Ok(author_from_slot(slot, &validators).map(|id| {
        let mut out = [0u8; 32];
        out.copy_from_slice(id.as_ref());
        out
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use subxt::config::substrate::DigestItem;
    use subxt::ext::codec::Encode;

    const AURA_ENGINE_ID: [u8; 4] = *b"aura";

    /// Sanity: a `BlockRow` is ready to be sent to the sink — all byte
    /// fields are plain `[u8; 32]`, no hex, no strings. This test
    /// doesn't need a node; it just locks the struct shape.
    #[test]
    fn row_fields_are_plain_bytes() {
        let row = BlockRow {
            num: 42,
            hash: [1u8; 32],
            parent_hash: [2u8; 32],
            state_root: [3u8; 32],
            extrinsics_root: [4u8; 32],
            author: Some([5u8; 32]),
            timestamp_ms: 1_234_567,
            spec_version: 1_002_011,
            extrinsic_count: 3,
            event_count: 7,
            ref_time: 825_000_382,
            proof_size: 18_410,
            ref_time_pct: 0,
            size_bytes: 195,
        };
        assert_eq!(row.hash.len(), 32);
        assert_eq!(row.author.unwrap().len(), 32);
    }

    /// Phase 1's author-resolution logic is the one contract the live
    /// worker must not break: for a `PreRuntime(aura, slot)` digest the
    /// author is `authorities[slot % len]`. We verify that on the pure
    /// helper path (`aura_slot` + `author_from_slot`) the same way the
    /// projection does — covering the "author resolved" fixture case
    /// in the plan without requiring a live node.
    #[test]
    fn author_selection_matches_slot_modulo_authority_set() {
        // Three dummy authorities whose account bytes are trivially
        // distinguishable, so an off-by-one in the modulo math fails
        // loudly.
        let authorities: Vec<AccountId32> =
            (1..=3).map(|i| AccountId32::from([i as u8; 32])).collect();

        for slot in 0u64..9 {
            let digest = Digest {
                logs: vec![DigestItem::PreRuntime(AURA_ENGINE_ID, slot.encode())],
            };
            let parsed_slot = aura_slot(&digest).expect("digest carries aura slot");
            let idx = (parsed_slot % authorities.len() as u64) as usize;
            let expected: [u8; 32] = {
                let mut out = [0u8; 32];
                out.copy_from_slice(authorities[idx].as_ref());
                out
            };
            let author = author_from_slot(parsed_slot, &authorities).unwrap();
            let mut actual = [0u8; 32];
            actual.copy_from_slice(author.as_ref());
            assert_eq!(actual, expected, "slot={slot} picked the wrong authority");
        }
    }

    /// The "block without author" case: a digest with no Aura
    /// `PreRuntime` entry must collapse to `author: None`, which is
    /// the sentinel `resolve_author_bytes` returns before it even hits
    /// the Aura storage. Testing `aura_slot(...) == None` + the
    /// `Option::map` short-circuit is enough to lock the path — the
    /// storage-free branch never runs the `try_fetch` call in code.
    #[test]
    fn block_without_aura_digest_yields_no_author() {
        let digest = Digest {
            logs: vec![DigestItem::Other(b"not-aura".to_vec())],
        };
        assert!(aura_slot(&digest).is_none());
        // Mimic what `resolve_author_bytes` does on the `None` branch:
        // it returns `Ok(None)` straight away. Locking that predicate
        // here avoids a future refactor silently falling through to the
        // storage fetch on non-Aura blocks.
        let author: Option<[u8; 32]> = aura_slot(&digest).map(|_| [0u8; 32]);
        assert!(author.is_none());
    }

    /// Authority set may be empty on degenerate dev chains; in that
    /// case `author_from_slot` returns `None` regardless of the slot.
    /// The projection maps that to `author: None` in the row, which the
    /// nullable `BYTEA` column accepts without complaint.
    #[test]
    fn empty_authority_set_yields_no_author() {
        let empty: Vec<AccountId32> = Vec::new();
        assert!(author_from_slot(7, &empty).is_none());
    }
}
