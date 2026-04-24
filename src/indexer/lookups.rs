//! ID resolvers used by the sink + query layers.
//!
//! Two cases the new schema needs before issuing any write:
//!
//! * `network_id TEXT` is interned into `SMALLINT` via the `networks`
//!   table — seeded once at boot from the compile-time
//!   [`crate::network::NETWORKS`] list.
//! * `author BYTEA` is interned into `INT` via the `authors` table —
//!   populated as the live / backfill workers encounter each
//!   validator for the first time.
//!
//! Why a dedicated module: the sink is meant to own every `INSERT`
//! statement, so any lookup that isn't just "bind this value" belongs
//! next to it. Keeping the HashMap caches out of `sink.rs` keeps that
//! file focused on row translation instead of dual-role resolver +
//! writer.
//!
//! Caching contract: `AuthorLookup` calls are outside the caller's
//! per-block transaction on purpose. An author row is *not* per-block
//! state — it's a stable per-validator registration that outlives any
//! single block insert. Writing through the pool (not the tx) means:
//!
//! * a block insert that rolls back never poisons the in-memory cache,
//! * two concurrent workers that see the same author at the same block
//!   race cleanly on `ON CONFLICT DO NOTHING`, and
//! * the next cache hit returns a valid, committed id even if the
//!   block that caused the resolution later failed.

use std::collections::HashMap;
use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::RwLock;

use crate::data::error::{DataError, DataResult};
use crate::network::NETWORKS;

/// Resolves `&str network_id` → `i16` for DB binds.
///
/// Seeded once at boot from the compile-time `NETWORKS` list, then
/// read-only for the process lifetime. Ids are 1-indexed (reserving 0
/// as a sentinel) and derived from the position of each network in
/// `NETWORKS` so they stay stable across environments without a
/// SERIAL sequence diverging between dev/staging/prod.
#[derive(Clone, Debug)]
pub struct NetworkLookup {
    by_name: HashMap<&'static str, i16>,
}

impl NetworkLookup {
    /// Upsert every compile-time network into the `networks` table
    /// and return the in-memory map. `ON CONFLICT DO NOTHING` makes
    /// the call idempotent — re-running it on an already-seeded DB is
    /// a no-op, and it's safe to call this from multiple processes
    /// racing on startup.
    pub async fn load_or_seed(pool: &PgPool) -> DataResult<Self> {
        let mut by_name: HashMap<&'static str, i16> = HashMap::with_capacity(NETWORKS.len());
        for (idx, net) in NETWORKS.iter().enumerate() {
            let sid: i16 = (idx as i16) + 1;
            sqlx::query(
                "INSERT INTO networks (id, name) VALUES ($1, $2) \
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(sid)
            .bind(net.id)
            .execute(pool)
            .await
            .map_err(|e| DataError::Rpc(format!("networks.seed({}): {e}", net.id)))?;
            by_name.insert(net.id, sid);
        }
        Ok(Self { by_name })
    }

    /// Look up the SMALLINT id for a network name. Returns `None` for
    /// an unregistered name — every caller should convert that into a
    /// clean error at its own boundary rather than panicking.
    pub fn resolve(&self, name: &str) -> Option<i16> {
        self.by_name.get(name).copied()
    }

    /// Convenience for hot paths that already validated the network
    /// is known. Panics (in debug) or returns 0 (in release) if the
    /// name isn't registered — callers should prefer [`resolve`] and
    /// handle the error cleanly.
    pub fn expect(&self, name: &str) -> i16 {
        self.by_name.get(name).copied().unwrap_or_else(|| {
            debug_assert!(false, "network_id {name:?} not registered in NetworkLookup");
            0
        })
    }
}

/// In-process cache of `(network_sid, author_bytes) → author_id`,
/// backed by the `authors` table. Miss path upserts the row and
/// records the id; concurrent misses on the same key are safe (the
/// DB serialises them on the unique index) and end up with matching
/// ids on both returning calls.
///
/// Wrapped in an `Arc<RwLock<_>>` so multiple workers can share a
/// single cache — ids are stable across networks (same `SERIAL`
/// sequence) but keys are scoped per-`(network_sid, bytes)`.
#[derive(Debug, Default)]
pub struct AuthorLookup {
    cache: RwLock<HashMap<(i16, [u8; 32]), i32>>,
}

impl AuthorLookup {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Resolve or upsert the `authors.id` for this validator on this
    /// network. Writes through the pool (not the caller's
    /// transaction) because the author row's lifetime is decoupled
    /// from the block insert's — see the module doc for the rationale.
    pub async fn resolve(
        &self,
        pool: &PgPool,
        network_sid: i16,
        bytes: [u8; 32],
    ) -> DataResult<i32> {
        let key = (network_sid, bytes);
        if let Some(id) = self.cache.read().await.get(&key).copied() {
            return Ok(id);
        }
        // Upsert then read back the id. `ON CONFLICT DO UPDATE SET
        // bytes = EXCLUDED.bytes` is a no-op rewrite whose only purpose
        // is to make `RETURNING id` fire on the conflicting row — the
        // alternative (`ON CONFLICT DO NOTHING` + fallback SELECT) costs
        // a round-trip on every concurrent miss.
        let id: i32 = sqlx::query_scalar(
            "INSERT INTO authors (network_id, bytes) VALUES ($1, $2) \
             ON CONFLICT (network_id, bytes) DO UPDATE SET bytes = EXCLUDED.bytes \
             RETURNING id",
        )
        .bind(network_sid)
        .bind(&bytes[..])
        .fetch_one(pool)
        .await
        .map_err(|e| DataError::Rpc(format!("authors.resolve(network_sid={network_sid}): {e}")))?;
        self.cache.write().await.insert(key, id);
        Ok(id)
    }

    /// Lookup-only path for readers: returns `None` when the author
    /// hasn't been seen yet. Used by the query layer to map an
    /// `author_id` back to its bytes without forcing a JOIN on every
    /// read.
    pub async fn get(&self, network_sid: i16, bytes: [u8; 32]) -> Option<i32> {
        self.cache.read().await.get(&(network_sid, bytes)).copied()
    }
}
