//! Indexer orchestration — the boot-side entry point for the live
//! workers and the backfill supervisors.
//!
//! Multi-tenant: one deployment indexes every configured network into
//! the same Postgres DB. [`spawn`] takes a list of `(network_id,
//! RpcClient)` pairs and creates one live worker + one backfill
//! supervisor per entry. They all share the single `PgPool`.
//!
//! The worker lifecycle is spawn-and-forget: the `JoinHandle`s are
//! returned so `main` can keep handles for shutdown ordering, but we
//! don't `.await` them on the critical path — the HTTP stack owns
//! process liveness. A worker failure logs and exits; the supervisor
//! inside [`crate::data::rpc::RpcClient`] already handles reconnects,
//! so transient RPC faults never need us to respawn the worker.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use sqlx::PgPool;
use tokio::task::JoinHandle;

use crate::data::rpc::RpcClient;

pub mod backfill;
pub mod buffer;
pub mod live;
pub mod lookups;
pub mod metadata;
pub mod projections;
pub mod reconcile;
pub mod sink;

/// Deployment role selected by the `--mode` CLI flag.
///
/// The three modes correspond to §"Topologie de déploiement" in
/// `docs/indexing-plan.md`:
///
/// * `All` — dev and early prod. A single process runs the indexer
///   workers *and* serves HTTP.
/// * `Indexer` — worker-only replica. Keeps writing to Postgres but
///   never opens an HTTP port.
/// * `Server` — HTTP-only replica. Reads Postgres, subscribes to the
///   finalized watch for live banner data, does not index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IndexerMode {
    #[default]
    All,
    Indexer,
    Server,
}

impl IndexerMode {
    /// Does this mode need to spawn the live worker?
    pub fn runs_indexer(self) -> bool {
        matches!(self, IndexerMode::All | IndexerMode::Indexer)
    }

    /// Does this mode serve HTTP?
    pub fn runs_http(self) -> bool {
        matches!(self, IndexerMode::All | IndexerMode::Server)
    }

    /// Must `DATABASE_URL` be set for this mode to make sense?
    ///
    /// Both `indexer` (which writes) and `server` (which reads) are
    /// useless without a database — the boot path refuses them outright
    /// so operators see the misconfiguration immediately instead of a
    /// process that silently no-ops. `all` tolerates a missing DB so
    /// early-dev / onboarding workflows still render pages via the RPC
    /// fallback.
    pub fn requires_database(self) -> bool {
        matches!(self, IndexerMode::Indexer | IndexerMode::Server)
    }
}

impl fmt::Display for IndexerMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            IndexerMode::All => "all",
            IndexerMode::Indexer => "indexer",
            IndexerMode::Server => "server",
        })
    }
}

impl FromStr for IndexerMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "all" => Ok(IndexerMode::All),
            "indexer" => Ok(IndexerMode::Indexer),
            "server" => Ok(IndexerMode::Server),
            other => Err(format!(
                "unknown --mode={other:?} (expected one of: all, indexer, server)"
            )),
        }
    }
}

/// Spawn the live + backfill workers for every indexed network in
/// `networks`. Returns one `JoinHandle` per spawned task.
///
/// Each entry `(network_id, rpc)` gets:
/// * one [`live::LiveWorker`] bound to that network and its client,
/// * one [`backfill::BackfillSupervisor`] (which itself spawns
///   `backfill_concurrency` workers + one seed task).
///
/// All tasks share the single `pool`, so writes are serialised through
/// the same connection pool but per-network sink guards (`ON CONFLICT
/// (network_id, …)`) keep them isolated.
///
/// `backfill_concurrency` is the per-network worker pool size; `0`
/// falls back to [`backfill::DEFAULT_CONCURRENCY`].
pub fn spawn(
    mode: IndexerMode,
    networks: Vec<(&'static str, Arc<RpcClient>)>,
    pool: PgPool,
    backfill_concurrency: usize,
    network_lookup: lookups::NetworkLookup,
    author_lookup: Arc<lookups::AuthorLookup>,
) -> Vec<JoinHandle<()>> {
    let mut handles = Vec::new();

    if !mode.runs_indexer() {
        tracing::info!(%mode, "indexer workers skipped (server-only mode)");
        return handles;
    }

    if networks.is_empty() {
        tracing::warn!(%mode, "indexer: no networks configured, no workers spawned");
        return handles;
    }

    let concurrency = if backfill_concurrency == 0 {
        backfill::DEFAULT_CONCURRENCY
    } else {
        backfill_concurrency
    };

    for (network_id, client) in networks {
        let Some(network_sid) = network_lookup.resolve(network_id) else {
            tracing::error!(
                %mode,
                network = network_id,
                "indexer: network_id not registered in networks table, skipping"
            );
            continue;
        };
        tracing::info!(
            %mode,
            network = network_id,
            network_sid,
            "spawning live worker"
        );
        handles.push(
            live::LiveWorker::new(
                network_id,
                network_sid,
                client.clone(),
                pool.clone(),
                author_lookup.clone(),
            )
            .spawn(),
        );

        tracing::info!(
            %mode,
            network = network_id,
            network_sid,
            concurrency,
            "spawning backfill supervisor"
        );
        handles.extend(
            backfill::BackfillSupervisor::new(
                network_id,
                network_sid,
                client,
                pool.clone(),
                author_lookup.clone(),
            )
            .with_concurrency(concurrency)
            .spawn(),
        );
    }

    handles
}
