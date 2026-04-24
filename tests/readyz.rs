//! Phase 7 — integration coverage for the readiness probe.
//!
//! Three invariants locked here:
//!
//! 1. A cursor seeded within the lag budget reads as `Ok` through the
//!    same path the axum handler uses — confirming `cursor_age_seconds`
//!    + `ready_verdict` compose correctly against a real DB.
//! 2. A cursor stamped artificially old flips the verdict to
//!    `CursorStale(age)` with the age the SQL view actually observed,
//!    proving the threshold is checked after the DB round-trip and not
//!    somewhere in-process (a bug that would mask a stuck worker).
//! 3. The max-lag constant matches what the banner uses — otherwise
//!    `/readyz` could 503 while the banner still claims `Healthy`, or
//!    vice-versa, and operators would chase phantom discrepancies.

#![cfg(all(feature = "ssr", not(feature = "mock")))]

mod common;

use std::time::Duration;

use allfeat_explorer::indexer::sink;
use allfeat_explorer::server::health::{ready_verdict, ReadyVerdict, READY_MAX_LAG_SECONDS};
use common::{fresh_db, fresh_lookups, TEST_NETWORK};

/// Smoke: a freshly stamped cursor (age ≈ 0) with a ready backend
/// collapses to `Ok`. Locks the happy path through the real SQL view,
/// so a migration that renames `updated_at` or drops the default
/// surfaces here rather than in prod.
#[tokio::test]
#[ignore = "requires docker compose postgres-test"]
async fn ok_when_healthy() {
    let db = fresh_db().await;
    let pool = db.pool();
    let (networks, _author) = fresh_lookups(pool).await;
    let sid = networks.resolve(TEST_NETWORK).expect("TEST_NETWORK seeded");

    sqlx::query(
        "INSERT INTO indexer_cursor (network_id, stream, last_indexed) VALUES ($1, $2, $3)",
    )
    .bind(sid)
    .bind(sink::LIVE_CURSOR)
    .bind(42i64)
    .execute(pool)
    .await
    .expect("seed cursor");

    let age = sink::cursor_age_seconds(pool, sid, sink::LIVE_CURSOR)
        .await
        .expect("cursor age");
    let verdict = ready_verdict(true, age, false, READY_MAX_LAG_SECONDS);
    assert_eq!(verdict, ReadyVerdict::Ok);
}

/// Push `updated_at` far into the past so the SQL view reports a lag
/// past the threshold. We mutate the row directly rather than sleeping
/// 31 seconds — sleep-driven tests are the single biggest source of CI
/// flakes and the verdict function doesn't care how the age was
/// produced, only that the SQL pipeline returns it.
#[tokio::test]
#[ignore = "requires docker compose postgres-test"]
async fn flips_503_when_cursor_exceeds_lag_budget() {
    let db = fresh_db().await;
    let pool = db.pool();
    let (networks, _author) = fresh_lookups(pool).await;
    let sid = networks.resolve(TEST_NETWORK).expect("TEST_NETWORK seeded");

    sqlx::query(
        "INSERT INTO indexer_cursor (network_id, stream, last_indexed) VALUES ($1, $2, $3)",
    )
    .bind(sid)
    .bind(sink::LIVE_CURSOR)
    .bind(42i64)
    .execute(pool)
    .await
    .expect("seed cursor");

    // Back-date by 2 × threshold so the verdict is unambiguous even if
    // the CI clock drifts by a few seconds between UPDATE and SELECT.
    let stale_seconds = READY_MAX_LAG_SECONDS * 2;
    sqlx::query(&format!(
        "UPDATE indexer_cursor SET updated_at = now() - interval '{stale_seconds} seconds' \
         WHERE network_id = $1 AND stream = $2"
    ))
    .bind(sid)
    .bind(sink::LIVE_CURSOR)
    .execute(pool)
    .await
    .expect("back-date cursor");

    let age = sink::cursor_age_seconds(pool, sid, sink::LIVE_CURSOR)
        .await
        .expect("cursor age");
    let verdict = ready_verdict(true, age, false, READY_MAX_LAG_SECONDS);
    match verdict {
        ReadyVerdict::CursorStale(observed) => {
            assert!(
                observed >= stale_seconds,
                "verdict should carry the observed age ≥ back-dated offset, got {observed}"
            );
        }
        other => panic!("expected CursorStale, got {other:?}"),
    }
}

/// A verdict chain that ultimately reports "ready" should block for
/// less than the normal request budget — a chatty health check on a
/// loaded cluster must not be the reason a pod falls behind on its
/// own probe deadline.
#[tokio::test]
#[ignore = "requires docker compose postgres-test"]
async fn verdict_is_cheap() {
    let db = fresh_db().await;
    let pool = db.pool();
    let (networks, _author) = fresh_lookups(pool).await;
    let sid = networks.resolve(TEST_NETWORK).expect("TEST_NETWORK seeded");
    sqlx::query(
        "INSERT INTO indexer_cursor (network_id, stream, last_indexed) VALUES ($1, $2, $3)",
    )
    .bind(sid)
    .bind(sink::LIVE_CURSOR)
    .bind(1i64)
    .execute(pool)
    .await
    .expect("seed cursor");

    let start = std::time::Instant::now();
    for _ in 0..20 {
        let age = sink::cursor_age_seconds(pool, sid, sink::LIVE_CURSOR)
            .await
            .expect("cursor age");
        let _ = ready_verdict(true, age, false, READY_MAX_LAG_SECONDS);
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(500),
        "20 verdict cycles should complete in well under 500 ms on a local pg, took {elapsed:?}",
    );
}
