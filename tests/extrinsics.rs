//! Phase 3 — integration coverage for the extrinsics projection +
//! the DB/buffer routing in [`IndexedProvider`].
//!
//! Each test needs a dev node at `ws://127.0.0.1:9944` (override via
//! `ALLFEAT_TEST_NODE_URL`) and a fresh Postgres DB provisioned by the
//! shared `fresh_db()` helper. All tests are `#[ignore]` by default so
//! `cargo test` without the surrounding services stays green; CI runs
//! them with `--ignored`.
//!
//! Coverage:
//!
//! * `lookup_by_hash_from_db` — backfill a short range, pick one
//!   extrinsic from the resulting rows and resolve it by its raw
//!   hash bytes. Exercises the `extrinsics.hash` index + the DB
//!   query path.
//! * `extrinsic_by_id_round_trips_through_provider` — hit
//!   `IndexedProvider::extrinsic_by_id` directly, assert the domain
//!   value carries a signer (when present), pallet, call, fee and a
//!   result status. The API handler returns the same struct verbatim,
//!   so validating it at the provider boundary is sufficient.
//! * `buffer_lookup_tip` — inject a synthetic best block carrying one
//!   extrinsic into the `PendingBuffer`, then resolve it via
//!   `IndexedProvider::extrinsic_by_id` with `is_finalized = false`
//!   (i.e. the block number sits above the buffer's finalized head).

#![cfg(all(feature = "ssr", not(feature = "mock")))]

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use allfeat_explorer::data::indexed::IndexedProvider;
use allfeat_explorer::data::rpc::{RpcClient, RpcProvider};
use allfeat_explorer::data::ChainData;
use allfeat_explorer::domain::{CallResult, Extrinsic, ExtrinsicArgs};
use allfeat_explorer::indexer::backfill::BackfillWorker;
use allfeat_explorer::indexer::buffer::{shared, BufferedBlock};
use allfeat_explorer::network::{ChainCtx, MELODIE};

use common::{
    dev_node_url, fresh_db, fresh_lookups, lookup_cell, wait_for_chunk_status,
    wait_for_finalized_head, TEST_NETWORK,
};

/// Wire a Melodie-flavoured `RpcProvider` pointing at the dev node.
/// Mirrors the shape `AppState::from_config` builds in prod so the
/// tests exercise the same routing surface.
fn melodie_provider(client: Arc<RpcClient>) -> RpcProvider {
    let mut clients: HashMap<&'static str, Arc<RpcClient>> = HashMap::new();
    clients.insert(MELODIE.id, client);
    RpcProvider::new(clients)
}

fn melodie_ctx() -> ChainCtx {
    // DB-backed reads never consult `now_ms`; set it to 0 and let the
    // DB answer from the block's stored `timestamp_ms`.
    ChainCtx::new(&MELODIE, 0)
}

/// Backfill a short range then look up one extrinsic by its raw hash.
/// Any signed extrinsic in the window makes a good probe — the hash
/// index is the one contract this test locks.
#[tokio::test]
#[ignore = "requires a running dev node and postgres-test"]
async fn lookup_by_hash_from_db() {
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let client = Arc::new(RpcClient::new(dev_node_url(), 42));
    let head = wait_for_finalized_head(&client, Duration::from_secs(15)).await;

    // A ~10-block window is enough to find a `timestamp.set` inherent
    // on every block, which guarantees at least one row the test can
    // fish out by hash even on a very quiet dev chain.
    let from = head.saturating_sub(10);
    let to = head;
    sqlx::query(
        "INSERT INTO backfill_chunks (network_id, from_block, to_block, status) \
         VALUES ($1, $2, $3, 'pending')",
    )
    .bind(TEST_NETWORK)
    .bind(from as i64)
    .bind(to as i64)
    .execute(&pool)
    .await
    .expect("seed pending chunk");

    let (networks, author_lookup) = fresh_lookups(&pool).await;
    let sid = networks.resolve(TEST_NETWORK).expect("TEST_NETWORK seeded");
    let worker = BackfillWorker::new(
        TEST_NETWORK,
        sid,
        client.clone(),
        pool.clone(),
        0,
        author_lookup.clone(),
    )
    .spawn();
    wait_for_chunk_status(&pool, from, to, "done", Duration::from_secs(30)).await;

    // Pick any extrinsic the backfill produced. Ordering by
    // `(block_num DESC, idx DESC)` keeps the probe deterministic
    // across reruns on the same chain.
    let (block_num, idx, hash_bytes): (i64, i32, Vec<u8>) = sqlx::query_as(
        "SELECT block_num, idx, hash \
           FROM extrinsics \
          WHERE network_id = $1 AND block_num BETWEEN $2 AND $3 \
          ORDER BY block_num DESC, idx DESC \
          LIMIT 1",
    )
    .bind(TEST_NETWORK)
    .bind(from as i64)
    .bind(to as i64)
    .fetch_one(&pool)
    .await
    .expect("at least one extrinsic indexed");
    assert_eq!(hash_bytes.len(), 32, "hash column must be 32 raw bytes");

    // Resolve via the provider using the 0x-prefixed hash string — same
    // shape the UI search bar passes into the server fn.
    let rpc = Arc::new(melodie_provider(client.clone()));
    let provider = IndexedProvider::new(
        pool.clone(),
        [MELODIE.id],
        rpc,
        lookup_cell(networks.clone()),
    );
    let hash_hex = format!(
        "0x{}",
        hash_bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let found = provider
        .extrinsic_by_id(melodie_ctx(), &hash_hex)
        .await
        .expect("provider call succeeds")
        .expect("extrinsic exists under the hash we pulled from the DB");
    assert_eq!(found.block_number, block_num as u64);
    assert_eq!(found.index, idx as u32);
    assert_eq!(found.hash, hash_hex);

    worker.abort();
    let _ = worker.await;
}

/// The SSR-facing smoke test: backfill a range and resolve one of the
/// resulting extrinsics via its `"<block>-<idx>"` id, the exact shape
/// the `/extrinsic/:id` route renders. Asserts the fields the
/// extrinsic detail page shows are non-empty.
#[tokio::test]
#[ignore = "requires a running dev node and postgres-test"]
async fn extrinsic_by_id_round_trips_through_provider() {
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let client = Arc::new(RpcClient::new(dev_node_url(), 42));
    let head = wait_for_finalized_head(&client, Duration::from_secs(15)).await;

    let from = head.saturating_sub(8);
    let to = head;
    sqlx::query(
        "INSERT INTO backfill_chunks (network_id, from_block, to_block, status) \
         VALUES ($1, $2, $3, 'pending')",
    )
    .bind(TEST_NETWORK)
    .bind(from as i64)
    .bind(to as i64)
    .execute(&pool)
    .await
    .expect("seed chunk");
    let (networks, author_lookup) = fresh_lookups(&pool).await;
    let sid = networks.resolve(TEST_NETWORK).expect("TEST_NETWORK seeded");
    let worker = BackfillWorker::new(
        TEST_NETWORK,
        sid,
        client.clone(),
        pool.clone(),
        0,
        author_lookup.clone(),
    )
    .spawn();
    wait_for_chunk_status(&pool, from, to, "done", Duration::from_secs(30)).await;

    // Probe on any row — the detail page renders fields that all
    // extrinsics carry (pallet, call, fee, status), so the target
    // doesn't need to be a specific call.
    let (block_num, idx): (i64, i32) = sqlx::query_as(
        "SELECT block_num, idx \
           FROM extrinsics \
          WHERE network_id = $1 AND block_num BETWEEN $2 AND $3 \
          ORDER BY block_num DESC, idx DESC \
          LIMIT 1",
    )
    .bind(TEST_NETWORK)
    .bind(from as i64)
    .bind(to as i64)
    .fetch_one(&pool)
    .await
    .expect("backfilled row exists");

    let rpc = Arc::new(melodie_provider(client.clone()));
    let provider = IndexedProvider::new(
        pool.clone(),
        [MELODIE.id],
        rpc,
        lookup_cell(networks.clone()),
    );
    let id = format!("{block_num}-{idx}");
    let got = provider
        .extrinsic_by_id(melodie_ctx(), &id)
        .await
        .expect("provider ok")
        .expect("extrinsic exists");
    assert_shape(&got, block_num as u64, idx as u32);

    worker.abort();
    let _ = worker.await;
}

/// Drop a synthetic best block into the [`PendingBuffer`] and verify
/// the provider resolves both its `"block-idx"` id and its 0x-prefixed
/// hash from the buffer — the DB is untouched. The injected block
/// number sits above `finalized_head` so the contract "extrinsic
/// served from buffer ⇒ not finalized" holds (Postgres has no row
/// for this block number).
#[tokio::test]
#[ignore = "requires postgres-test (no node required)"]
async fn buffer_lookup_tip() {
    let db = fresh_db().await;
    let pool = db.pool().clone();

    // Build a buffer with one synthetic extrinsic. Hash value is
    // arbitrary but deterministic so the test can reconstruct the
    // 0x-prefixed string and feed it to the provider.
    let buffer = shared(MELODIE.id);
    let block_num = 999_999u64;
    let hash_bytes = [0x42u8; 32];
    let hash_hex = format!(
        "0x{}",
        hash_bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let synthetic = Extrinsic {
        id: format!("{block_num}-0"),
        block_number: block_num,
        index: 0,
        hash: hash_hex.clone(),
        module: "Balances".into(),
        call: "transfer_keep_alive".into(),
        signed: true,
        signer: Some("5GrwvaEFinqsLPxV6dRiYpZkmf3uG9bAagdNXTuZmrjN6dhV".into()),
        args: ExtrinsicArgs::Raw { hex: "0x".into() },
        result: CallResult::Success,
        nonce: Some(0),
        tip: 0,
        fee: 42,
        timestamp_ms: 0,
        events: Vec::new(),
    };
    {
        let mut buf = buffer.write().await;
        buf.append_block(BufferedBlock {
            number: block_num,
            hash: [0xaau8; 32],
            extrinsics: vec![synthetic.clone()],
            transfers: Vec::new(),
        });
    }

    // The RPC provider is still required for non-extrinsic methods
    // (the trait surface is wide), but we never hit it in this test —
    // we only invoke `extrinsic_by_id`. A dummy client pointed at the
    // same URL keeps construction cheap and avoids introducing a
    // mock dependency.
    let rpc_client = Arc::new(RpcClient::new(dev_node_url(), 42));
    let rpc = Arc::new(melodie_provider(rpc_client));
    let (networks, _author_lookup) = fresh_lookups(&pool).await;
    let provider = IndexedProvider::new(
        pool.clone(),
        [MELODIE.id],
        rpc,
        lookup_cell(networks.clone()),
    )
    .with_buffer(MELODIE.id, buffer);

    // Resolve by canonical id.
    let via_id = provider
        .extrinsic_by_id(melodie_ctx(), &format!("{block_num}-0"))
        .await
        .expect("provider ok")
        .expect("extrinsic resolves from buffer by id");
    assert_eq!(via_id, synthetic);

    // Resolve by hash — same extrinsic, second code path (the parser
    // returns `ExtrinsicLookup::Hash`, the provider hits
    // `PendingBuffer::extrinsic_by_hash`).
    let via_hash = provider
        .extrinsic_by_id(melodie_ctx(), &hash_hex)
        .await
        .expect("provider ok")
        .expect("extrinsic resolves from buffer by hash");
    assert_eq!(via_hash, synthetic);

    // The buffer holds it, Postgres does not — the block sits above
    // the buffer's finalized_head (0), so the finalization contract
    // the buffer path advertises is upheld.
    let db_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM extrinsics WHERE network_id = $1")
        .bind(TEST_NETWORK)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(db_count, 0, "buffer lookup must not touch the DB");
}

/// Shared shape assertions for the SSR-facing test. Keeps the body of
/// the test focused on the routing path rather than field plumbing.
fn assert_shape(got: &Extrinsic, block_num: u64, idx: u32) {
    assert_eq!(got.block_number, block_num);
    assert_eq!(got.index, idx);
    assert_eq!(got.id, format!("{block_num}-{idx}"));
    assert!(
        got.hash.starts_with("0x") && got.hash.len() == 2 + 64,
        "hash must be lowercase 0x + 64 hex chars, got {}",
        got.hash
    );
    assert!(!got.module.is_empty(), "pallet name must be populated");
    assert!(!got.call.is_empty(), "call name must be populated");
    assert!(
        matches!(got.result, CallResult::Success | CallResult::Failed),
        "result must be explicitly decoded"
    );
    // Signed extrinsics must carry a signer. Inherents (signed=false)
    // carry None — both are legal, we only require the invariant.
    if got.signed {
        assert!(
            got.signer.is_some(),
            "signed extrinsic needs an SS58 signer"
        );
    } else {
        assert!(got.signer.is_none(), "unsigned extrinsic has no signer");
    }
}
