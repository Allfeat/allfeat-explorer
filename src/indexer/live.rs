//! Live worker — the finalized-stream consumer.
//!
//! One live worker instance per indexed network. Each subscribes to its
//! own RPC client's finalized-head watch channel (fed by the existing
//! `RpcClient` supervisor — no second subscription to maintain), and
//! for each new head projects + inserts the corresponding `blocks`
//! row plus its derived projections (extrinsics, events, balances,
//! ATS). All writes carry the worker's `network_id`.
//!
//! Restart / resume semantics:
//!
//! * At boot, read `indexer_cursor` for `(network_id, 'live')` → `last`.
//!   `None` means the first observed finalized head becomes the new
//!   cursor (no retroactive history walk — that's the backfill's job).
//! * Each watch tick exposes the chain's current finalized head. If a
//!   gap opened (`head > last + 1`), we fill it sequentially.
//! * Any projection / insert failure is logged and the loop breaks out
//!   of the current gap fill; the next watch tick retries from the same
//!   `last`. The RPC-layer supervisor handles transport recovery.
//! * `ON CONFLICT (network_id, num) DO NOTHING` in the sink makes replay
//!   free: a restart with a cursor one block behind the DB's real head
//!   simply re-inserts that block as a no-op.

use std::sync::Arc;

use sqlx::PgPool;
use tokio::task::JoinHandle;

use crate::data::error::{DataError, DataResult};
use crate::data::rpc::client::race_timeout;
use crate::data::rpc::mappers::accounts::fetch_accounts_at;
use crate::data::rpc::RpcClient;

use super::{projections, sink};

/// Live worker owning its RPC client reference, its pool, and its
/// network id. Spawned by [`super::spawn`]; the struct is public so
/// integration tests can build + start one directly without going
/// through the CLI.
pub struct LiveWorker {
    client: Arc<RpcClient>,
    pool: PgPool,
    network_id: &'static str,
    network_sid: i16,
    author_lookup: Arc<super::lookups::AuthorLookup>,
}

impl LiveWorker {
    pub fn new(
        network_id: &'static str,
        network_sid: i16,
        client: Arc<RpcClient>,
        pool: PgPool,
        author_lookup: Arc<super::lookups::AuthorLookup>,
    ) -> Self {
        Self {
            client,
            pool,
            network_id,
            network_sid,
            author_lookup,
        }
    }

    /// Spawn the worker on the current tokio runtime and return its
    /// `JoinHandle`. Errors inside the run loop are logged and the
    /// worker exits — the RPC supervisor already handles connection
    /// churn, and a crashed worker never corrupts DB state because
    /// every block is indexed in its own transaction.
    pub fn spawn(self) -> JoinHandle<()> {
        tokio::spawn(async move {
            let net = self.network_id;
            if let Err(e) = self.run().await {
                tracing::error!(network = net, error = %e, "live worker exited with error");
            }
        })
    }

    /// Run the watch loop. Exits cleanly when every watch sender has
    /// been dropped (the `RpcClient` is gone), returns `Err` only when
    /// boot-time plumbing fails (DB unreachable, cursor row corrupted).
    pub async fn run(self) -> DataResult<()> {
        // Kick the RPC connection so the supervisor is definitely
        // running before we start waiting on its channel. Without this
        // a brand-new `RpcClient` never spawns its watcher (the
        // watcher is started inside `subxt()`), and our `changed()`
        // below would hang forever.
        self.client.subxt().await?;

        let mut rx = self.client.watch_finalized_head();
        let mut last = sink::load_cursor(&self.pool, self.network_sid, sink::LIVE_CURSOR).await?;
        tracing::info!(network = self.network_id, cursor = ?last, "live worker started");

        loop {
            // `borrow_and_update` snapshots the current value AND
            // marks the receiver as "caught up". Without the update,
            // the next `changed()` would immediately return for the
            // value we already processed and we'd spin.
            let head_now = *rx.borrow_and_update();
            if let Some(head) = head_now {
                let start = match last {
                    Some(x) => x.saturating_add(1),
                    // Fresh DB: anchor the cursor at the current head
                    // instead of walking back to genesis (that's the
                    // backfill's job). We still index `head` itself so
                    // the integration test sees a row at least as soon
                    // as the next finalized block lands.
                    None => head,
                };
                for num in start..=head {
                    if let Err(e) = self.index_block(num).await {
                        tracing::warn!(
                            network = self.network_id,
                            error = %e,
                            number = num,
                            "live index failed; will retry on next head"
                        );
                        break;
                    }
                    last = Some(num);
                }
            }

            if rx.changed().await.is_err() {
                tracing::info!(
                    network = self.network_id,
                    "finalized watch closed; live worker shutting down"
                );
                return Ok(());
            }
        }
    }

    /// Index one finalized block: fetch, project, insert, bump cursor
    /// — all in a single sqlx transaction so a crash between the block
    /// row and the cursor bump can't stamp the cursor ahead of reality.
    ///
    /// The `account_balances` UPSERT reads `System::Account` directly
    /// at this block's hash for every touched account, then writes
    /// absolute values. That's idempotent under replay (same block →
    /// same reading → same write) and robust against whichever pallet
    /// ecosystem mutated balances on chain — pallet_balances events
    /// are just the hint for *which* accounts to refetch, not the
    /// source of truth for their balance. The RPC fan-out runs outside
    /// the sqlx transaction so we don't pin a DB connection while
    /// waiting on the node.
    async fn index_block(&self, num: u64) -> DataResult<()> {
        let api = self.client.subxt().await?;
        let at = race_timeout("at_block", api.at_block(num))
            .await?
            .map_err(|e| DataError::Rpc(format!("at_block({num}): {e}")))?;
        let row = projections::blocks::map(&at, num).await?;
        let extrinsic_rows = projections::extrinsics::map(&at, num).await?;
        let event_rows = projections::events::map(&at, num).await?;
        let balance_projection =
            projections::balances::project_block(&at, &extrinsic_rows, num).await?;
        let touched = projections::accounts::collect_touched(
            &balance_projection.touched_accounts,
            &extrinsic_rows,
        );
        let snapshots = fetch_accounts_at(&at, &touched).await?;
        let ats_ops = projections::ats::project_block(&at, &extrinsic_rows, num).await?;
        // Genesis has no balance events — the initial supply is laid
        // down by the chain spec. Walk `System::Account` once so every
        // endowed account lands in the DB; regular blocks 1..N then
        // keep individual rows fresh via the per-block touched set.
        let genesis_snapshots = if num == 0 {
            projections::genesis::project_genesis_accounts(&at).await?
        } else {
            Vec::new()
        };

        let net = self.network_id;
        let sid = self.network_sid;
        let block_ts = row.timestamp_ms;

        // Resolve author_id outside the transaction — `AuthorLookup`
        // owns a pool connection of its own so a block rollback can't
        // poison its in-memory cache (see lookups.rs module doc).
        let author_id = match row.author {
            Some(bytes) => Some(self.author_lookup.resolve(&self.pool, sid, bytes).await?),
            None => None,
        };

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| DataError::Rpc(format!("begin tx ({net}/block {num}): {e}")))?;
        sink::insert_block(&mut tx, sid, &row, author_id).await?;
        sink::insert_extrinsics(&mut tx, sid, block_ts, &extrinsic_rows).await?;
        sink::insert_events(&mut tx, sid, block_ts, &event_rows).await?;
        sink::insert_balance_movements(&mut tx, sid, &balance_projection.movements).await?;
        sink::apply_account_snapshots(&mut tx, sid, num, block_ts, &snapshots).await?;
        if num == 0 {
            sink::apply_account_snapshots(&mut tx, sid, 0, block_ts, &genesis_snapshots).await?;
        }
        sink::apply_ats(&mut tx, sid, &ats_ops).await?;
        sink::bump_cursor(&mut tx, sid, sink::LIVE_CURSOR, num).await?;
        tx.commit()
            .await
            .map_err(|e| DataError::Rpc(format!("commit tx ({net}/block {num}): {e}")))?;

        // Count every committed block, even a `fresh = false` replay —
        // the counter tracks work the process did, and a dashboard
        // watching it wants to see that a crash-recovered worker is
        // still turning the crank.
        crate::server::metrics::record_block_indexed(net, crate::server::metrics::STREAM_LIVE);
        tracing::debug!(
            network = net,
            number = num,
            spec = row.spec_version,
            "indexed block"
        );
        Ok(())
    }
}
