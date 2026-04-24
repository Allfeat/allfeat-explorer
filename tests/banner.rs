//! Phase 0.5 / Phase 1 — integration coverage for the indexer status
//! derivation consumed by the UI banner.
//!
//! `reflects_live_lag` exercises the wiring: once the cursor falls
//! behind the chain tip, the banner must flip to `CatchingUp` with the
//! correct `live_lag_blocks`. We read the cursor the same way the API
//! handler does — injecting a fake row via the `indexer_cursor` table
//! and composing with `compute_status` — so any future refactor to the
//! cursor schema fails loudly here.
//!
//! The former `endpoint_returns_healthy_stub` asserted a Leptos-only
//! short-circuit (empty Vec when the server-fn context was missing);
//! the v3 rewrite replaces that path with an Axum handler and the test
//! is re-introduced as an HTTP-level fixture in Phase 2.

#![cfg(feature = "ssr")]

mod common;

use std::time::Duration;

use allfeat_explorer::indexer::sink;
use allfeat_explorer::server::health::{compute_status, IndexerState};
use common::{fresh_db, fresh_lookups, TEST_NETWORK};

/// A cursor row sitting several hundred blocks behind the chain tip
/// must surface as `CatchingUp` with the matching `live_lag_blocks`.
/// We stamp the cursor artificially (no live worker) and compose with
/// the pure helper the server fn uses — that covers the two failure
/// modes we care about: the SQL read path drifting from the schema,
/// and the state-derivation threshold silently changing.
#[tokio::test]
#[ignore = "requires docker compose postgres-test"]
async fn reflects_live_lag() {
    let db = fresh_db().await;
    let pool = db.pool();
    let (networks, _author) = fresh_lookups(pool).await;
    let sid = networks.resolve(TEST_NETWORK).expect("TEST_NETWORK seeded");

    // Inject a cursor 400 blocks behind the chain tip. Using the real
    // INSERT path (not a fixture row) means any migration change that
    // adds NOT NULL columns to `indexer_cursor` breaks this test too,
    // not just the production worker.
    sqlx::query(
        "INSERT INTO indexer_cursor (network_id, stream, last_indexed) VALUES ($1, $2, $3)",
    )
    .bind(sid)
    .bind(sink::LIVE_CURSOR)
    .bind(100i64)
    .execute(pool)
    .await
    .expect("seed cursor row");

    let indexer_head = sink::load_cursor(pool, sid, sink::LIVE_CURSOR)
        .await
        .expect("load_cursor");
    let cursor_age = sink::cursor_age_seconds(pool, sid, sink::LIVE_CURSOR)
        .await
        .expect("cursor_age");

    // The cursor was just inserted, so its age should be tiny — far
    // under the Offline threshold. Lock that so a future schema shift
    // that changes the `updated_at` default doesn't push us into
    // Offline territory by accident.
    assert!(
        cursor_age.unwrap_or(i64::MAX) < 5,
        "freshly stamped cursor must be young (< 5s), got {cursor_age:?}",
    );

    // Simulate the chain tip 400 blocks ahead of our cursor.
    let status = compute_status(TEST_NETWORK, Some(500), indexer_head, cursor_age, 0, 0);

    assert_eq!(status.state, IndexerState::CatchingUp);
    assert_eq!(status.live_lag_blocks, Some(400));
    assert_eq!(status.indexer_head, Some(100));
    assert_eq!(status.finalized_head, Some(500));

    // Stretch the simulated age past the offline threshold: state
    // must flip to `Offline` without touching the cursor row itself,
    // locking the priority of "stuck cursor" over "big lag" in the
    // decision tree.
    let stale = compute_status(TEST_NETWORK, Some(500), indexer_head, Some(120), 0, 0);
    assert_eq!(stale.state, IndexerState::Offline);

    // Sanity: waiting for a cursor we already seeded resolves
    // instantly — `wait_for_cursor` is used by the live-worker tests
    // and we want a smoke-check here that it's not silently hanging
    // on the shared seed row.
    common::wait_for_cursor(pool, sink::LIVE_CURSOR, 100, Duration::from_secs(1)).await;
}
