//! Application state shared across every Axum handler.
//!
//! Built once at boot from [`ServerConfig`] and handed to the router as
//! its `State<AppState>` — handlers pull the pieces they need via the
//! standard Axum extractor.
//!
//! Multi-tenant indexing: every network whose `RPC_ENDPOINT_<ID>` env var
//! is set gets one entry in `indexer_clients`. Networks without an endpoint
//! configured are considered disabled — they never reach the indexer, the
//! RPC provider, or the UI switcher.
//!
//! The concrete provider is picked at compile time: `feature = "mock"`
//! links [`crate::data::mock::MockProvider`] and excludes the subxt
//! stack entirely; without the feature the explorer talks to real
//! nodes via [`crate::data::rpc::RpcProvider`].

use std::sync::Arc;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

use crate::data::ChainData;
use crate::live::encoder::EncoderHub;

use super::config::ServerConfig;

#[cfg(not(feature = "mock"))]
use std::collections::HashMap;
#[cfg(not(feature = "mock"))]
use std::sync::OnceLock;

#[cfg(not(feature = "mock"))]
use crate::data::rpc::RpcClient;
#[cfg(not(feature = "mock"))]
use crate::indexer::lookups::{AuthorLookup, NetworkLookup};

#[derive(Clone)]
pub struct AppState {
    pub provider: Arc<dyn ChainData>,
    pub config: Arc<ServerConfig>,
    /// Postgres pool shared by every indexed network. `None` when
    /// `DATABASE_URL` is unset — the pre-indexer RPC fallback still
    /// works in that state. Created lazily so a missing or unreachable
    /// DB doesn't prevent boot (the first real query surfaces the
    /// failure instead).
    pub db_pool: Option<PgPool>,
    /// RPC clients for every indexed network, keyed by
    /// `NetworkSpec::id`. The indexer workers (`crate::indexer::live`
    /// + `crate::indexer::backfill`) iterate this map; the banner /
    ///   status endpoint reads per-network finalized heads off each
    ///   client's watch channel. Empty in the mock build.
    #[cfg(not(feature = "mock"))]
    pub indexer_clients: HashMap<&'static str, Arc<RpcClient>>,
    /// Network ids this deployment serves — the subset of
    /// `crate::network::NETWORKS` whose `RPC_ENDPOINT_<ID>` env var was
    /// set at boot. Materialised as a stable slice so iteration order
    /// matches the `NETWORKS` declaration order (not `HashMap` insertion
    /// order). `&'static`: every id comes from the static catalogue.
    /// Empty in `mock` builds — the mock UI reads the full catalogue
    /// directly.
    pub indexed_network_ids: Arc<[&'static str]>,
    /// Resolves `&str network_id` → `SMALLINT` via the `networks`
    /// lookup table. Shared with [`IndexedProvider`] at construction
    /// via `Arc<OnceLock<_>>`; both read the same populated value once
    /// [`AppState::seed_lookups`] fires post-migration. Stays empty in
    /// mock / DB-less builds — every reader gates on
    /// [`OnceLock::get`] being `Some`.
    #[cfg(not(feature = "mock"))]
    pub network_lookup: Arc<OnceLock<Arc<NetworkLookup>>>,
    /// Per-process cache of (network_sid, author_bytes) → author_id,
    /// backed by the `authors` lookup table. Always constructed (the
    /// empty cache is harmless in mock / DB-less builds); the workers
    /// that feed it only run when a pool is present.
    #[cfg(not(feature = "mock"))]
    pub author_lookup: Arc<AuthorLookup>,
    /// Shared encoder/fanout for the live WebSocket. One task per
    /// `(network, topic)` combo encodes each item once and rebroadcasts
    /// the finished `Message`, so N subscribers don't trigger N
    /// redundant `serde_json` passes per event. Lazy: entries are born
    /// on first subscribe and torn down once the last session drops its
    /// lease.
    pub encoder_hub: Arc<EncoderHub>,
}

impl AppState {
    pub fn from_config(config: ServerConfig) -> Self {
        // `connect_lazy` is intentional: the first query is what opens a
        // real connection, so operators see DB errors at the query site
        // (with an actionable stack) instead of a generic boot failure.
        let db_pool = config.database_url.as_deref().map(|url| {
            PgPoolOptions::new()
                .max_connections(16)
                .connect_lazy(url)
                .expect("DATABASE_URL is not a valid Postgres connection string")
        });

        #[cfg(feature = "mock")]
        let (provider, indexed_network_ids): (Arc<dyn ChainData>, Arc<[&'static str]>) = (
            Arc::new(crate::data::mock::MockProvider::new()),
            Arc::from(Vec::<&'static str>::new()),
        );

        #[cfg(not(feature = "mock"))]
        let network_lookup: Arc<OnceLock<Arc<NetworkLookup>>> = Arc::new(OnceLock::new());

        #[cfg(not(feature = "mock"))]
        let (provider, indexer_clients, indexed_network_ids) = {
            use crate::data::indexed::IndexedProvider;
            use crate::data::rpc::RpcProvider;

            // Walk `NETWORKS` (not `rpc_endpoints`) so the resulting
            // `indexed_network_ids` slice preserves declaration order —
            // matters for the banner rows and the switcher, both of
            // which iterate this list verbatim. Networks whose env var
            // wasn't set are skipped here and stay invisible everywhere.
            let mut clients: HashMap<&'static str, Arc<RpcClient>> = HashMap::new();
            let mut indexed_ids: Vec<&'static str> = Vec::new();
            for net in crate::network::NETWORKS {
                if let Some(url) = config.rpc_endpoints.get(net.id) {
                    clients.insert(
                        net.id,
                        Arc::new(RpcClient::new(url.clone(), net.ss58_prefix)),
                    );
                    indexed_ids.push(net.id);
                }
            }
            let rpc = Arc::new(RpcProvider::new(clients.clone()));

            // When a DB is configured, route every indexed network
            // through the DB provider — it only takes over the
            // migrated methods and falls back to `rpc` for the rest,
            // so a cold DB still serves every page. Hand the provider
            // a clone of the OnceLock so that once `seed_lookups`
            // populates it, every query call site sees the SMALLINT
            // mapping without a second plumbing pass.
            let provider: Arc<dyn ChainData> = match db_pool.clone() {
                Some(pool) => Arc::new(IndexedProvider::new(
                    pool,
                    indexed_ids.iter().copied(),
                    rpc.clone(),
                    network_lookup.clone(),
                )),
                None => rpc.clone(),
            };
            (provider, clients, Arc::from(indexed_ids))
        };

        Self {
            provider,
            config: Arc::new(config),
            db_pool,
            #[cfg(not(feature = "mock"))]
            indexer_clients,
            indexed_network_ids,
            #[cfg(not(feature = "mock"))]
            network_lookup,
            #[cfg(not(feature = "mock"))]
            author_lookup: Arc::new(AuthorLookup::default()),
            encoder_hub: EncoderHub::new(),
        }
    }

    /// Populate [`Self::network_lookup`] from the `networks` table.
    /// Called once at boot *after* migrations run, so the table
    /// already exists. No-op when the deployment has no DB configured
    /// (the lookup stays empty and every caller falls through to the
    /// RPC provider). Safe to call with `&self` since the underlying
    /// `OnceLock` handles write synchronisation.
    #[cfg(not(feature = "mock"))]
    pub async fn seed_lookups(&self) -> crate::data::error::DataResult<()> {
        if let Some(pool) = self.db_pool.as_ref() {
            let lookup = NetworkLookup::load_or_seed(pool).await?;
            // `set` returns `Err` only if another caller raced us —
            // which would be a logic bug (boot calls this exactly
            // once). Ignore the error; either value is correct.
            let _ = self.network_lookup.set(Arc::new(lookup));
        }
        Ok(())
    }
}
