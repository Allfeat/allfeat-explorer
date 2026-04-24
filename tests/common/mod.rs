//! Shared test fixtures for integration suites.
//!
//! All DB-touching tests use [`fresh_db`] to obtain a jetable Postgres
//! database: a brand-new `test_<pid>_<rand>` DB on the test instance,
//! migrations applied, connection pool ready. `Drop` schedules the
//! teardown (`DROP DATABASE`), so assertions can panic without leaking
//! state across runs.
//!
//! The admin URL defaults to the `postgres-test` service from
//! `docker-compose.yml` (port 54329). CI overrides via
//! `TEST_DATABASE_URL`. It MUST point at a Postgres role allowed to
//! `CREATE DATABASE`; the suite never runs against the primary dev
//! database.

#![cfg(feature = "ssr")]
#![allow(dead_code)] // not every integration test uses every helper

use std::time::{Duration, Instant};

use allfeat_explorer::indexer::lookups::{AuthorLookup, NetworkLookup};
use allfeat_explorer::server::MIGRATOR;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, Executor, PgConnection, PgPool};
use std::sync::Arc;

/// Per-test database handle. The pool and the server the pool points at
/// are guaranteed to be alive for the lifetime of this value — the
/// `Drop` impl schedules the `DROP DATABASE` on a detached thread so the
/// DB is gone by the next `fresh_db()` call.
pub struct TestDb {
    pool: Option<PgPool>,
    name: String,
    admin_url: String,
}

impl TestDb {
    /// Connection pool for the per-test DB.
    pub fn pool(&self) -> &PgPool {
        self.pool
            .as_ref()
            .expect("TestDb pool is only None during Drop")
    }

    /// Name of the per-test DB — useful for assertions that need to
    /// reference the DB explicitly (e.g. `pg_database` lookups).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Full connection string for the per-test DB. Useful for
    /// sub-processes (sqlx migrate CLI, other binaries) that need to
    /// reconnect from scratch.
    pub fn url(&self) -> String {
        replace_db_in_url(&self.admin_url, &self.name)
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        // Take ownership of the pool so we can close it on the teardown
        // thread — Postgres rejects `DROP DATABASE` while sessions are
        // still attached, so closing must run before the DROP.
        let pool = self.pool.take();
        let admin_url = std::mem::take(&mut self.admin_url);
        let name = std::mem::take(&mut self.name);

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("teardown runtime");
            rt.block_on(async move {
                if let Some(p) = pool {
                    p.close().await;
                }
                if let Err(e) = drop_database(&admin_url, &name).await {
                    eprintln!("test db teardown failed for {name}: {e}");
                }
            });
        });
    }
}

/// Provision a brand-new database, apply the full migration set, and
/// return a pool connected to it.
pub async fn fresh_db() -> TestDb {
    let admin_url = test_admin_url();
    let name = generate_db_name();

    // Admin connection only exists long enough to issue `CREATE
    // DATABASE`. Postgres doesn't allow `CREATE DATABASE` inside a
    // transaction, so we bypass the pool and use a single connection.
    let mut admin = PgConnection::connect(&admin_url)
        .await
        .unwrap_or_else(|e| panic!("connect to test admin DB at {admin_url}: {e}"));
    admin
        .execute(format!("CREATE DATABASE \"{name}\"").as_str())
        .await
        .unwrap_or_else(|e| panic!("create DB {name}: {e}"));
    admin.close().await.expect("close admin connection");

    let url = replace_db_in_url(&admin_url, &name);
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&url)
        .await
        .unwrap_or_else(|e| panic!("connect to new DB {name}: {e}"));

    MIGRATOR
        .run(&pool)
        .await
        .unwrap_or_else(|e| panic!("run migrations on {name}: {e}"));

    TestDb {
        pool: Some(pool),
        name,
        admin_url,
    }
}

/// Seed the `networks` lookup table on a freshly-migrated DB and hand
/// back a [`NetworkLookup`] + empty [`AuthorLookup`]. Integration tests
/// that drive the sink / queries directly need both to pass the correct
/// SMALLINT network_id.
pub async fn fresh_lookups(pool: &PgPool) -> (NetworkLookup, Arc<AuthorLookup>) {
    let networks = NetworkLookup::load_or_seed(pool)
        .await
        .unwrap_or_else(|e| panic!("seed NetworkLookup: {e}"));
    (networks, Arc::new(AuthorLookup::default()))
}

/// Shorthand: resolve `TEST_NETWORK` → SMALLINT. Tests that only query
/// the default network skip the full lookup dance.
pub async fn test_network_sid(pool: &PgPool) -> i16 {
    let (lookup, _) = fresh_lookups(pool).await;
    lookup
        .resolve(TEST_NETWORK)
        .unwrap_or_else(|| panic!("TEST_NETWORK={TEST_NETWORK:?} not registered in networks"))
}

/// Wrap `NetworkLookup` in an `Arc<OnceLock<...>>` primed with the
/// resolved lookup — matches the shape `IndexedProvider::new` expects.
/// Tests that build a provider directly should thread this through.
pub fn lookup_cell(lookup: NetworkLookup) -> Arc<std::sync::OnceLock<Arc<NetworkLookup>>> {
    let cell = Arc::new(std::sync::OnceLock::new());
    let _ = cell.set(Arc::new(lookup));
    cell
}

/// Connection string for the admin DB (the `postgres` database on the
/// test instance). Overridable via `TEST_DATABASE_URL` for CI jobs that
/// run Postgres on a non-default port.
pub fn test_admin_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://explorer:explorer@127.0.0.1:54329/postgres".to_string())
}

fn generate_db_name() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    // PID narrows us to one binary; the nanos-since-epoch plus a
    // per-process atomic counter handles the "two tests started
    // within the same nanosecond" race that a plain `subsec_nanos()`
    // would miss. The counter also covers the case where the system
    // clock has second-level resolution (some CI hypervisors).
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("test_{pid}_{nanos}_{seq}")
}

fn replace_db_in_url(url: &str, new_db: &str) -> String {
    // `postgres://user:pw@host:port/dbname?opts` → swap the segment
    // between the last `/` (after host) and the optional `?`. The URL
    // crate would be overkill for a test helper.
    let (head, tail) = match url.rsplit_once('/') {
        Some(split) => split,
        None => return url.to_string(),
    };
    let query = tail.find('?').map(|i| &tail[i..]).unwrap_or("");
    format!("{head}/{new_db}{query}")
}

/// Dev-node WebSocket used by the integration suites. Defaults to the
/// conventional `ws://127.0.0.1:9944` dev binary; CI and remote dev
/// override via `ALLFEAT_TEST_NODE_URL`.
pub fn dev_node_url() -> String {
    std::env::var("ALLFEAT_TEST_NODE_URL").unwrap_or_else(|_| "ws://127.0.0.1:9944".to_string())
}

/// Network id every integration test uses unless it explicitly
/// exercises cross-network behaviour. Hard-coded on purpose: tests
/// target Allfeat's dev node, and a fixed id keeps wait helpers /
/// assertions simple.
pub const TEST_NETWORK: &str = "allfeat";

/// Poll `blocks` for a row at `num` on `TEST_NETWORK` until it exists.
pub async fn wait_for_block(pool: &PgPool, num: u64, timeout: Duration) {
    wait_for_block_on(pool, TEST_NETWORK, num, timeout).await
}

/// Poll `blocks` for a row at `num` on `network_id`. Explicit-network
/// variant for suites that exercise multi-tenant routing.
///
/// The per-row `network_id` column is now `SMALLINT`, so the WHERE
/// clause resolves the caller's string id through the `networks`
/// lookup sub-select. Keeps the test helper API string-based.
pub async fn wait_for_block_on(pool: &PgPool, network_id: &str, num: u64, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(\
                SELECT 1 FROM blocks \
                WHERE network_id = (SELECT id FROM networks WHERE name = $1) \
                  AND num = $2)",
        )
        .bind(network_id)
        .bind(num as i64)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|e| panic!("wait_for_block({network_id}/{num}): {e}"));
        if exists {
            return;
        }
        if Instant::now() >= deadline {
            panic!("block {network_id}/{num} did not appear within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Poll `indexer_cursor` for `TEST_NETWORK` / `stream`.
pub async fn wait_for_cursor(pool: &PgPool, stream: &str, at_least: u64, timeout: Duration) {
    wait_for_cursor_on(pool, TEST_NETWORK, stream, at_least, timeout).await
}

/// Poll `indexer_cursor` for `(network_id, stream)`.
pub async fn wait_for_cursor_on(
    pool: &PgPool,
    network_id: &str,
    stream: &str,
    at_least: u64,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    loop {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT last_indexed FROM indexer_cursor \
              WHERE network_id = (SELECT id FROM networks WHERE name = $1) \
                AND stream = $2",
        )
        .bind(network_id)
        .bind(stream)
        .fetch_optional(pool)
        .await
        .unwrap_or_else(|e| panic!("wait_for_cursor({network_id}/{stream}): {e}"));
        if let Some((v,)) = row {
            if v as u64 >= at_least {
                return;
            }
        }
        if Instant::now() >= deadline {
            panic!("cursor {network_id}/{stream} did not reach {at_least} within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Poll `backfill_chunks.status` for `(TEST_NETWORK, from, to)`.
pub async fn wait_for_chunk_status(
    pool: &PgPool,
    from: u64,
    to: u64,
    want: &str,
    timeout: Duration,
) {
    wait_for_chunk_status_on(pool, TEST_NETWORK, from, to, want, timeout).await
}

/// Poll `backfill_chunks.status` for `(network_id, from, to)`.
pub async fn wait_for_chunk_status_on(
    pool: &PgPool,
    network_id: &str,
    from: u64,
    to: u64,
    want: &str,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    loop {
        let row: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT status, last_error
               FROM backfill_chunks
              WHERE network_id = (SELECT id FROM networks WHERE name = $1)
                AND from_block = $2 AND to_block = $3",
        )
        .bind(network_id)
        .bind(from as i64)
        .bind(to as i64)
        .fetch_optional(pool)
        .await
        .unwrap_or_else(|e| panic!("wait_for_chunk_status({network_id}/{from},{to}): {e}"));
        if let Some((status, _)) = &row {
            if status == want {
                return;
            }
        }
        if Instant::now() >= deadline {
            panic!(
                "chunk {network_id}/({from},{to}) never reached status={want:?} within {timeout:?}, saw {:?}",
                row
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Wait until the RPC client's finalized-head watch publishes a value,
/// then return it. Required because backfill tests need a real chain
/// tip to target; starting a worker without waiting hits
/// `finalized_head() == None` and the worker idles instead of indexing.
#[cfg(not(feature = "mock"))]
pub async fn wait_for_finalized_head(
    client: &allfeat_explorer::data::rpc::RpcClient,
    timeout: Duration,
) -> u64 {
    let deadline = Instant::now() + timeout;
    let mut rx = client.watch_finalized_head();
    // Prime the connection so the watcher task actually spawns — without
    // this, `changed()` waits forever on a channel nobody writes to.
    let _ = client.subxt().await;
    loop {
        if let Some(head) = *rx.borrow() {
            return head;
        }
        let now = Instant::now();
        if now >= deadline {
            panic!("finalized head never fired within {timeout:?}");
        }
        let remaining = deadline - now;
        match tokio::time::timeout(remaining, rx.changed()).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => panic!("finalized-head watch channel closed unexpectedly"),
            Err(_) => panic!("finalized head never fired within {timeout:?}"),
        }
    }
}

async fn drop_database(admin_url: &str, name: &str) -> Result<(), sqlx::Error> {
    let mut conn = PgConnection::connect(admin_url).await?;
    // `WITH (FORCE)` terminates lingering sessions rather than failing
    // — safer for abrupt test exits.
    conn.execute(format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)").as_str())
        .await?;
    conn.close().await?;
    Ok(())
}
