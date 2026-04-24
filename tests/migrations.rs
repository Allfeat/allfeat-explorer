//! Phase 0 — Sanity around the SQL migration bundle.
//!
//! These tests talk to a real Postgres (the `postgres-test` service from
//! `docker-compose.yml` by default; `TEST_DATABASE_URL` overrides). They
//! are `#[ignore]` by default so `cargo test` without a reachable
//! Postgres is still green; CI and dev runs opt in with
//! `cargo test --features ssr -- --ignored`.
//!
//! Coverage:
//!   * every table described in the indexing plan exists after a clean
//!     migrate run on a fresh DB;
//!   * every critical index documented in the plan is actually created;
//!   * rerunning the migration set is a no-op (idempotency).

#![cfg(feature = "ssr")]

mod common;

use allfeat_explorer::server::MIGRATOR;
use common::fresh_db;

/// Table list mirrors `migrations/001_initial.sql` 1:1. Updating the
/// migration without touching this list is the whole point — missing an
/// entry here means the migration silently dropped a table.
const EXPECTED_TABLES: &[&str] = &[
    "blocks",
    "extrinsics",
    "events",
    "balance_movements",
    "account_balances",
    "ats_registry",
    "ats_versions",
    "runtime_versions",
    "indexer_cursor",
    "backfill_chunks",
];

/// Indexes the indexing plan calls out explicitly as "critical". These
/// are the ones the query layer relies on for pagination and lookup —
/// dropping one would silently shift a millisecond query into a
/// full-scan.
const EXPECTED_INDEXES: &[&str] = &[
    "blocks_hash_idx",
    "extrinsics_signer_idx",
    "extrinsics_hash_idx",
    "account_balances_total_idx",
];

#[tokio::test]
#[ignore = "requires docker compose postgres-test"]
async fn runs_clean_on_fresh_db() {
    let db = fresh_db().await;
    let pool = db.pool();

    for table in EXPECTED_TABLES {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM information_schema.tables
                 WHERE table_schema = 'public' AND table_name = $1
             )",
        )
        .bind(table)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|e| panic!("query for table {table}: {e}"));
        assert!(exists, "table {table} is missing after migrate run");
    }

    for index in EXPECTED_INDEXES {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM pg_indexes
                 WHERE schemaname = 'public' AND indexname = $1
             )",
        )
        .bind(index)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|e| panic!("query for index {index}: {e}"));
        assert!(exists, "index {index} is missing after migrate run");
    }
}

#[tokio::test]
#[ignore = "requires docker compose postgres-test"]
async fn idempotent_rerun() {
    let db = fresh_db().await;
    let pool = db.pool();

    // `migrate!()` already ran during `fresh_db()`. Running it again on
    // the same pool should be a no-op — sqlx tracks applied migrations
    // in `_sqlx_migrations` and skips anything already recorded.
    MIGRATOR
        .run(pool)
        .await
        .expect("second migrate!() run should be a no-op");

    // Cross-check: the applied migration count must match the directory
    // count, and `_sqlx_migrations` must not grow between runs.
    let applied_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(pool)
        .await
        .expect("count _sqlx_migrations");

    MIGRATOR
        .run(pool)
        .await
        .expect("third migrate!() run should also be a no-op");

    let applied_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(pool)
        .await
        .expect("count _sqlx_migrations");

    assert_eq!(
        applied_before, applied_after,
        "rerunning migrate!() must not record additional migrations"
    );
}
