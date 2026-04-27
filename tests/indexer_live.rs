//! Phase 1 — integration coverage for the live worker.
//!
//! Both tests talk to a real Allfeat-compatible dev node at
//! `ws://127.0.0.1:9944` (override via `ALLFEAT_TEST_NODE_URL`) and a
//! fresh Postgres database from `docker-compose.yml::postgres-test`.
//! They are `#[ignore]` by default so `cargo test` without the
//! surrounding services still stays green; CI runs them with
//! `--ignored`.
//!
//! Scope:
//!
//! * `indexes_new_finalized_block` — spawn the worker against a clean
//!   DB, wait until it commits at least one finalized block, verify
//!   the row has a plausible 32-byte hash.
//! * `resumes_from_cursor_after_restart` — simulate a stale cursor
//!   (rewound by one block) and assert that the replay hits the
//!   `ON CONFLICT DO NOTHING` guard on `blocks` without panicking the
//!   worker, then climbs past the original cursor on the next head.

#![cfg(feature = "ssr")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use allfeat_explorer::data::rpc::RpcClient;
use allfeat_explorer::network::RuntimeKind;
use allfeat_explorer::indexer::live::LiveWorker;
use allfeat_explorer::indexer::sink;

use common::{
    dev_node_url, fresh_db, fresh_lookups, wait_for_block, wait_for_cursor, TEST_NETWORK,
};

#[tokio::test]
#[ignore = "requires a running dev node and postgres-test"]
async fn indexes_new_finalized_block() {
    let db = fresh_db().await;
    let pool = db.pool().clone();

    let (networks, author_lookup) = fresh_lookups(&pool).await;
    let sid = networks.resolve(TEST_NETWORK).expect("TEST_NETWORK seeded");
    let client = Arc::new(RpcClient::new(dev_node_url(), TEST_NETWORK, 42, RuntimeKind::Allfeat));
    let handle = LiveWorker::new(
        TEST_NETWORK,
        sid,
        client.clone(),
        pool.clone(),
        author_lookup.clone(),
    )
    .spawn();

    // `0` is a safe lower bound on a dev chain (genesis finalizes
    // quickly), and the helper short-circuits as soon as the cursor
    // row appears. 15 s is the plan's budget for a finalized tick.
    wait_for_cursor(&pool, sink::LIVE_CURSOR, 0, Duration::from_secs(15)).await;

    let block_num: i64 = sqlx::query_scalar(
        "SELECT last_indexed FROM indexer_cursor WHERE network_id = $1 AND stream = $2",
    )
    .bind(TEST_NETWORK)
    .bind(sink::LIVE_CURSOR)
    .fetch_one(&pool)
    .await
    .expect("read cursor after first block");

    wait_for_block(&pool, block_num as u64, Duration::from_secs(15)).await;

    let (hash, extrinsic_count, spec_version): (Vec<u8>, i32, i32) = sqlx::query_as(
        "SELECT hash, extrinsic_count, spec_version FROM blocks WHERE network_id = $1 AND num = $2",
    )
    .bind(TEST_NETWORK)
    .bind(block_num)
    .fetch_one(&pool)
    .await
    .expect("row for the indexed block");

    // `BYTEA` round-trips as a byte vector — the projection writes a
    // 32-byte array, so anything else here is a schema / binding drift.
    assert_eq!(hash.len(), 32, "block hash should be 32 bytes");
    assert!(
        hash.iter().any(|b| *b != 0),
        "block hash must not be all-zero"
    );
    // The dev chain usually has at least the timestamp inherent.
    assert!(
        extrinsic_count >= 0,
        "extrinsic_count should never be negative"
    );
    assert!(spec_version > 0, "spec_version should be populated");

    handle.abort();
    let _ = handle.await;
}

#[tokio::test]
#[ignore = "requires a running dev node and postgres-test"]
async fn resumes_from_cursor_after_restart() {
    let db = fresh_db().await;
    let pool = db.pool().clone();

    // First worker: index at least one block, then shut it down.
    let (networks, author_lookup) = fresh_lookups(&pool).await;
    let sid = networks.resolve(TEST_NETWORK).expect("TEST_NETWORK seeded");
    let client = Arc::new(RpcClient::new(dev_node_url(), TEST_NETWORK, 42, RuntimeKind::Allfeat));
    let first = LiveWorker::new(
        TEST_NETWORK,
        sid,
        client.clone(),
        pool.clone(),
        author_lookup.clone(),
    )
    .spawn();
    wait_for_cursor(&pool, sink::LIVE_CURSOR, 0, Duration::from_secs(15)).await;
    let first_cursor: i64 = sqlx::query_scalar(
        "SELECT last_indexed FROM indexer_cursor WHERE network_id = $1 AND stream = $2",
    )
    .bind(sid)
    .bind(sink::LIVE_CURSOR)
    .fetch_one(&pool)
    .await
    .expect("read first cursor");
    first.abort();
    let _ = first.await;

    // Force a replay scenario: rewind the cursor by one block.
    if first_cursor > 0 {
        sqlx::query(
            "UPDATE indexer_cursor SET last_indexed = $1 \
                 WHERE network_id = $2 AND stream = $3",
        )
        .bind(first_cursor - 1)
        .bind(sid)
        .bind(sink::LIVE_CURSOR)
        .execute(&pool)
        .await
        .expect("rewind cursor");
    }

    // Second worker: should clear the replay AND move past the
    // original cursor on the next finalized head.
    let second = LiveWorker::new(
        TEST_NETWORK,
        sid,
        client.clone(),
        pool.clone(),
        author_lookup.clone(),
    )
    .spawn();
    wait_for_cursor(
        &pool,
        sink::LIVE_CURSOR,
        (first_cursor + 1) as u64,
        Duration::from_secs(30),
    )
    .await;

    // Blocks count must be at least 2 (first_cursor's row + one newer).
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM blocks WHERE network_id = $1")
        .bind(TEST_NETWORK)
        .fetch_one(&pool)
        .await
        .expect("count blocks");
    assert!(
        count >= 2,
        "indexing must keep moving after a stale-cursor restart, got {count} rows",
    );

    // And the bumped-cursor guard must hold.
    let new_cursor: i64 = sqlx::query_scalar(
        "SELECT last_indexed FROM indexer_cursor WHERE network_id = $1 AND stream = $2",
    )
    .bind(TEST_NETWORK)
    .bind(sink::LIVE_CURSOR)
    .fetch_one(&pool)
    .await
    .expect("read second cursor");
    assert!(
        new_cursor >= first_cursor,
        "cursor must not regress after replay; first={first_cursor}, new={new_cursor}",
    );

    second.abort();
    let _ = second.await;
}
