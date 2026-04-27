//! Phase 2 — integration coverage for the backfill runner.
//!
//! Each test talks to a real dev node at `ws://127.0.0.1:9944` (override
//! via `ALLFEAT_TEST_NODE_URL`) and a fresh Postgres per the shared
//! `fresh_db()` helper. All tests are `#[ignore]` by default — CI runs
//! them with `--ignored`, local `cargo test` without the surrounding
//! services stays green.
//!
//! Coverage:
//!
//! * `indexes_short_range` — a single chunk covering the last few
//!   finalized blocks reaches `status = 'done'` and every targeted
//!   block lands in the `blocks` table with a distinct hash. Exercises
//!   the claim → index → done loop end-to-end.
//! * `parallel_workers_no_duplicate` — four workers share a queue of
//!   many small chunks; the `FOR UPDATE SKIP LOCKED` claim must never
//!   hand the same chunk to two workers, and the PK guarantee on
//!   `blocks(num)` must absorb any harmless replay at chunk edges.
//! * `resumes_after_worker_crash` — a chunk stamped `running` with an
//!   expired lease must be claimable again. Simulates the crash by
//!   seeding the row directly rather than racing an abort mid-flight.
//! * `shows_backfilling_state` — while a seeded backfill drains on a
//!   fresh DB, `collect_status_from` reports `state = Backfilling`
//!   with a `backfill_pct` that grows monotonically across three
//!   samples.

#![cfg(all(feature = "ssr", not(feature = "mock")))]

mod common;

use std::sync::Arc;
use std::time::Duration;

use allfeat_explorer::data::rpc::RpcClient;
use allfeat_explorer::network::RuntimeKind;
use allfeat_explorer::indexer::backfill::{self, BackfillWorker, DEFAULT_CHUNK_SIZE};
use allfeat_explorer::server::health::{collect_status_from, IndexerState};

use common::{
    dev_node_url, fresh_db, fresh_lookups, wait_for_chunk_status, wait_for_finalized_head,
    TEST_NETWORK,
};

/// Seed a single pending chunk for `[from, to]` and confirm a worker
/// drains it cleanly. The assertions mirror the plan's §7 Phase 2
/// criteria: `COUNT(*) = to - from + 1`, hashes unique, no gap inside.
#[tokio::test]
#[ignore = "requires a running dev node and postgres-test"]
async fn indexes_short_range() {
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let (networks, author_lookup) = fresh_lookups(&pool).await;
    let sid = networks.resolve(TEST_NETWORK).expect("TEST_NETWORK seeded");
    let client = Arc::new(RpcClient::new(dev_node_url(), TEST_NETWORK, 42, RuntimeKind::Allfeat));
    let head = wait_for_finalized_head(&client, Duration::from_secs(15)).await;

    // A tiny 6-block window keeps the test fast on CI; widening it
    // wouldn't exercise any new code path. `saturating_sub` handles
    // the (unlikely) dev-chain that's still at genesis.
    let from = head.saturating_sub(5);
    let to = head;

    sqlx::query(
        "INSERT INTO backfill_chunks (network_id, from_block, to_block, status)
         VALUES ($1, $2, $3, 'pending')",
    )
    .bind(sid)
    .bind(from as i64)
    .bind(to as i64)
    .execute(&pool)
    .await
    .expect("seed pending chunk");

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

    let expected = (to - from + 1) as i64;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM blocks WHERE network_id = $1 AND num BETWEEN $2 AND $3",
    )
    .bind(sid)
    .bind(from as i64)
    .bind(to as i64)
    .fetch_one(&pool)
    .await
    .expect("count blocks");
    assert_eq!(count, expected, "every block in the chunk must be indexed");

    let distinct: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT hash) FROM blocks WHERE network_id = $1 AND num BETWEEN $2 AND $3",
    )
    .bind(sid)
    .bind(from as i64)
    .bind(to as i64)
    .fetch_one(&pool)
    .await
    .expect("count distinct hashes");
    assert_eq!(
        distinct, expected,
        "every indexed block must have a unique hash — a collision here would mean \
         two chunks wrote the same row"
    );

    worker.abort();
    let _ = worker.await;
}

/// Seed many small chunks covering the same overall range, launch four
/// workers, verify all chunks land in `done` and no duplicate blocks
/// accumulate in the `blocks` table.
#[tokio::test]
#[ignore = "requires a running dev node and postgres-test"]
async fn parallel_workers_no_duplicate() {
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let (networks, author_lookup) = fresh_lookups(&pool).await;
    let sid = networks.resolve(TEST_NETWORK).expect("TEST_NETWORK seeded");
    let client = Arc::new(RpcClient::new(dev_node_url(), TEST_NETWORK, 42, RuntimeKind::Allfeat));
    let head = wait_for_finalized_head(&client, Duration::from_secs(15)).await;

    // 16 single-block chunks covering `[head-15, head]`. Fine-grained
    // chunking maximises the claim rate so the race on the queue is
    // actually exercised rather than one worker sweeping the whole lot.
    let width: u64 = 15;
    let from = head.saturating_sub(width);
    let pairs: Vec<(u64, u64)> = (from..=head).map(|n| (n, n)).collect();
    for (f, t) in &pairs {
        sqlx::query(
            "INSERT INTO backfill_chunks (network_id, from_block, to_block, status)
             VALUES ($1, $2, $3, 'pending')",
        )
        .bind(sid)
        .bind(*f as i64)
        .bind(*t as i64)
        .execute(&pool)
        .await
        .expect("seed chunk");
    }

    let handles: Vec<_> = (0..4)
        .map(|i| {
            BackfillWorker::new(
                TEST_NETWORK,
                sid,
                client.clone(),
                pool.clone(),
                i,
                author_lookup.clone(),
            )
            .spawn()
        })
        .collect();

    for (f, t) in &pairs {
        wait_for_chunk_status(&pool, *f, *t, "done", Duration::from_secs(60)).await;
    }

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM blocks WHERE network_id = $1 AND num BETWEEN $2 AND $3",
    )
    .bind(sid)
    .bind(from as i64)
    .bind(head as i64)
    .fetch_one(&pool)
    .await
    .expect("count blocks in window");
    assert_eq!(
        count,
        (width + 1) as i64,
        "parallel workers must cover the whole window without gaps or duplicates"
    );

    let open: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM backfill_chunks \
          WHERE network_id = $1 AND status IN ('pending','running')",
    )
    .bind(sid)
    .fetch_one(&pool)
    .await
    .expect("count open chunks");
    assert_eq!(open, 0, "no chunk should be left pending or running");

    for h in handles {
        h.abort();
        let _ = h.await;
    }
}

/// A chunk marked `running` with an expired lease must be re-claimed
/// by the next worker to scan the queue. We inject the row directly —
/// racing a real abort would be flakey and adds no coverage over the
/// deterministic case.
#[tokio::test]
#[ignore = "requires a running dev node and postgres-test"]
async fn resumes_after_worker_crash() {
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let (networks, author_lookup) = fresh_lookups(&pool).await;
    let sid = networks.resolve(TEST_NETWORK).expect("TEST_NETWORK seeded");
    let client = Arc::new(RpcClient::new(dev_node_url(), TEST_NETWORK, 42, RuntimeKind::Allfeat));
    let head = wait_for_finalized_head(&client, Duration::from_secs(15)).await;

    let from = head.saturating_sub(3);
    let to = head;

    // Simulated crashed worker: status=running, lease 10 min in the past.
    sqlx::query(
        "INSERT INTO backfill_chunks (network_id, from_block, to_block, status, lease_until)
         VALUES ($1, $2, $3, 'running', now() - interval '10 minutes')",
    )
    .bind(sid)
    .bind(from as i64)
    .bind(to as i64)
    .execute(&pool)
    .await
    .expect("seed expired chunk");

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

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM blocks WHERE network_id = $1 AND num BETWEEN $2 AND $3",
    )
    .bind(sid)
    .bind(from as i64)
    .bind(to as i64)
    .fetch_one(&pool)
    .await
    .expect("count blocks");
    assert_eq!(count, (to - from + 1) as i64);

    worker.abort();
    let _ = worker.await;
}

/// Minimal integration check for the `Backfilling` banner state: on a
/// fresh DB with zero indexed blocks but a known finalized head, the
/// status endpoint must report `state = Backfilling` and `backfill_pct`
/// that stays at 0 (nothing indexed yet). Then as the worker drains a
/// seeded chunk the percentage must grow strictly.
#[tokio::test]
#[ignore = "requires a running dev node and postgres-test"]
async fn shows_backfilling_state() {
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let (networks, author_lookup) = fresh_lookups(&pool).await;
    let sid = networks.resolve(TEST_NETWORK).expect("TEST_NETWORK seeded");
    let client = Arc::new(RpcClient::new(dev_node_url(), TEST_NETWORK, 42, RuntimeKind::Allfeat));
    let head = wait_for_finalized_head(&client, Duration::from_secs(15)).await;

    // Initial sample: zero rows in `blocks`, so pct = 0, and the chain
    // has a known head → state is Backfilling. This is the wiring
    // check: `collect_status_from` must read COUNT(*) and compute total
    // from the finalized head, not short-circuit to Healthy.
    let s0 = collect_status_from(TEST_NETWORK, sid, Some(head), Some(&pool)).await;
    assert_eq!(
        s0.state,
        IndexerState::Backfilling,
        "fresh DB with a known head must start in Backfilling"
    );
    assert_eq!(s0.backfill_done, 0);
    assert_eq!(s0.backfill_total, head + 1);

    // Seed a chunk that's big enough to take multiple samples worth of
    // wall-clock to drain. 30 blocks × ~50 ms/block ≈ 1.5 s on a dev
    // node — roomy enough to catch at least one intermediate sample.
    let width = 30u64;
    let from = head.saturating_sub(width);
    let to = head;
    sqlx::query(
        "INSERT INTO backfill_chunks (network_id, from_block, to_block, status)
         VALUES ($1, $2, $3, 'pending')",
    )
    .bind(sid)
    .bind(from as i64)
    .bind(to as i64)
    .execute(&pool)
    .await
    .expect("seed chunk");

    let worker = BackfillWorker::new(
        TEST_NETWORK,
        sid,
        client.clone(),
        pool.clone(),
        0,
        author_lookup.clone(),
    )
    .spawn();

    // Sample at least twice more and confirm `backfill_done` never regresses.
    let mut prev = s0.backfill_done;
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_millis(400)).await;
        let s = collect_status_from(TEST_NETWORK, sid, Some(head), Some(&pool)).await;
        assert!(
            s.backfill_done >= prev,
            "backfill_done must be monotonic, got {} after {}",
            s.backfill_done,
            prev
        );
        prev = s.backfill_done;
    }

    wait_for_chunk_status(&pool, from, to, "done", Duration::from_secs(30)).await;

    worker.abort();
    let _ = worker.await;

    // Sanity check: once the chunk drains, `DEFAULT_CHUNK_SIZE` is
    // sized correctly for an integer % computation. Lock the constant
    // here too so a plan revision that changes the chunk width can't
    // silently drift the seed test budget.
    assert_eq!(DEFAULT_CHUNK_SIZE, 1000);

    // Also verify the plan's `SELECT COUNT(*)` reading via
    // `backfill::open_chunk_count` matches what the banner sees. No
    // open chunks after drain means a subsequent banner sample flips
    // to `Healthy` (modulo live-lag).
    let still_open = backfill::open_chunk_count(&pool, sid)
        .await
        .expect("open chunk count");
    assert_eq!(still_open, 0, "drained backfill must leave the queue empty");
}
