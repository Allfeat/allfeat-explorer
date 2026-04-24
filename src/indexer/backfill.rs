//! Backfill runner — parallel range workers that claim chunks out of
//! `backfill_chunks` and index historic blocks through the same
//! projection pipeline the live worker uses.
//!
//! Each worker pool is bound to a single `network_id`: one pool per
//! indexed chain. Chunks are scoped per network via the `(network_id,
//! from_block, to_block)` UNIQUE, so two pools never race on each
//! other's rows.
//!
//! Shape (Phase 2 of `docs/indexing-plan.md`):
//!
//! * [`split_gaps_into_chunks`] is the pure decomposition primitive:
//!   given the finalized head and the sorted set of blocks already
//!   present in Postgres for that network, it returns the `(from, to)`
//!   ranges still needing indexing, each capped at [`DEFAULT_CHUNK_SIZE`].
//! * [`seed_chunks`] materialises that list into `backfill_chunks` rows
//!   with `ON CONFLICT (network_id, from_block, to_block) DO NOTHING`.
//! * [`BackfillWorker`] loops: [`claim_chunk`] (atomic `FOR UPDATE SKIP
//!   LOCKED` scoped by `network_id`), index every block in the range,
//!   [`mark_chunk_done`] on success or [`mark_chunk_failed`] on error.
//! * [`BackfillSupervisor`] seeds once at boot then spawns the worker
//!   pool; workers never exit by themselves.
//!
//! Resume semantics: a worker that dies mid-chunk leaves the row in
//! `status = 'running'` with `lease_until = now() + LEASE_DURATION`.
//! After the lease expires any other worker (of the same network) can
//! claim it again.

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::task::JoinHandle;

use crate::data::error::{DataError, DataResult};
use crate::data::rpc::client::race_timeout;
use crate::data::rpc::mappers::accounts::fetch_accounts_at;
use crate::data::rpc::RpcClient;

use super::{projections, sink};

/// Maximum width of one chunk. The plan caps it at 1000 so no single
/// worker monopolises the queue and a stuck chunk only loses at most a
/// thousand blocks of progress on restart.
pub const DEFAULT_CHUNK_SIZE: u64 = 1000;

/// How long a claimed chunk stays owned by its worker before any other
/// worker may steal it. Five minutes is generous enough that a slow
/// archive node round-trip doesn't trigger a steal, yet short enough
/// that a crashed worker doesn't block a chunk for an operator-visible
/// delay.
pub const LEASE_DURATION: Duration = Duration::from_secs(5 * 60);

/// Default worker pool size. Picked by the plan as a balance between
/// throughput (~40 blocks/s with 4 workers on a local node) and load on
/// the upstream RPC.
pub const DEFAULT_CONCURRENCY: usize = 4;

/// How long a worker sleeps between empty-queue polls. Short enough that
/// the backfill finishes promptly once chunks are seeded, long enough
/// to not hot-loop the DB if the queue stays empty (e.g. post-drain).
const IDLE_POLL: Duration = Duration::from_secs(2);

/// How long [`BackfillSupervisor::seed_from_head`] waits for the RPC
/// watcher to publish a finalized head before giving up and leaving
/// the queue empty. Long enough for cold dev nodes that take a few
/// seconds to produce their first finalized block.
const SEED_HEAD_GRACE: Duration = Duration::from_secs(30);

/// Last-error column is `TEXT`, uncapped. We cap ourselves at 2 KiB so
/// a runaway decode panic doesn't push a megabyte of garbage per row
/// into the backfill table.
const LAST_ERROR_CAP: usize = 2048;

/// One chunk owned by the current worker for the duration of its
/// indexing pass. Produced by [`claim_chunk`], consumed by the worker
/// loop, then retired via [`mark_chunk_done`] or [`mark_chunk_failed`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimedChunk {
    pub id: i64,
    pub from_block: u64,
    pub to_block: u64,
}

/// Partition `[0, head]` minus `existing_sorted` into `(from, to)`
/// inclusive ranges no wider than `chunk_size`.
///
/// Contract:
///
/// * `existing_sorted` MUST be ascending. Out-of-range or regressive
///   entries are tolerated (skipped) but won't be re-sorted — the
///   caller is responsible for feeding a monotonically increasing
///   sequence, e.g. via `ORDER BY num`.
/// * `chunk_size` MUST be > 0. A zero-width chunk size would generate
///   infinite chunks; we panic instead of silently hanging the boot.
/// * The returned ranges are ordered ascending and non-overlapping.
///   Concatenating them yields exactly the set of missing blocks.
pub fn split_gaps_into_chunks(
    head: u64,
    existing_sorted: &[u64],
    chunk_size: u64,
) -> Vec<(u64, u64)> {
    assert!(chunk_size > 0, "chunk_size must be > 0");
    let mut chunks = Vec::new();
    let mut cursor: u64 = 0;

    for &present in existing_sorted {
        if present > head || present < cursor {
            continue;
        }
        if present > cursor {
            push_chunked(&mut chunks, cursor, present - 1, chunk_size);
        }
        cursor = present.saturating_add(1);
    }
    if cursor <= head {
        push_chunked(&mut chunks, cursor, head, chunk_size);
    }
    chunks
}

/// Split `[from, to]` into consecutive `(from, to)` slices no wider than
/// `chunk_size`. Pushed directly into `out` so the caller doesn't pay
/// for an intermediate allocation per gap.
fn push_chunked(out: &mut Vec<(u64, u64)>, from: u64, to: u64, chunk_size: u64) {
    debug_assert!(chunk_size > 0);
    let mut f = from;
    loop {
        let end = f.saturating_add(chunk_size - 1).min(to);
        out.push((f, end));
        if end == to {
            break;
        }
        f = end + 1;
    }
}

/// Compute the gap set for `[0, head]` on `network_id` and INSERT one
/// `pending` row per chunk into `backfill_chunks`. Returns the number
/// of *new* rows (existing rows skipped via the UNIQUE aren't counted).
///
/// Idempotent: safe to call on every boot.
pub async fn seed_chunks(
    pool: &PgPool,
    network_sid: i16,
    head: u64,
    chunk_size: u64,
) -> DataResult<u64> {
    // 3 M u64 ≈ 24 MiB — acceptable for a one-shot boot scan. If the DB
    // ever grows past that threshold swap this for a server-side gap
    // computation (the plan sketches one in §2).
    let raw: Vec<i64> = sqlx::query_scalar(
        "SELECT num FROM blocks WHERE network_id = $1 AND num <= $2 ORDER BY num",
    )
    .bind(network_sid)
    .bind(head as i64)
    .fetch_all(pool)
    .await
    .map_err(|e| {
        DataError::Rpc(format!(
            "seed_chunks(net={network_sid}): load existing blocks: {e}"
        ))
    })?;
    let existing: Vec<u64> = raw.into_iter().map(|v| v as u64).collect();

    let chunks = split_gaps_into_chunks(head, &existing, chunk_size);
    let planned = chunks.len();

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| DataError::Rpc(format!("seed_chunks(net={network_sid}): begin tx: {e}")))?;
    let mut inserted: u64 = 0;
    for (from, to) in chunks {
        let res = sqlx::query(
            "INSERT INTO backfill_chunks (network_id, from_block, to_block, status)
             VALUES ($1, $2, $3, 'pending')
             ON CONFLICT (network_id, from_block, to_block) DO NOTHING",
        )
        .bind(network_sid)
        .bind(from as i64)
        .bind(to as i64)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            DataError::Rpc(format!(
                "seed_chunks(net={network_sid}): insert ({from},{to}): {e}"
            ))
        })?;
        inserted += res.rows_affected();
    }
    tx.commit()
        .await
        .map_err(|e| DataError::Rpc(format!("seed_chunks(net={network_sid}): commit: {e}")))?;

    tracing::info!(
        network_sid,
        head,
        planned,
        inserted,
        "backfill seed: partitioned missing blocks into chunks"
    );
    Ok(inserted)
}

/// Atomically claim one pending (or crashed-running) chunk for
/// `network_id`. Returns `None` when the per-network queue is drained.
///
/// The inner `SELECT … FOR UPDATE SKIP LOCKED` is the canonical
/// Postgres work-queue idiom: two workers racing on the same scan
/// never pick the same row, and a row locked by a running worker is
/// invisible to its peers until the lock releases or the lease expires.
pub async fn claim_chunk(pool: &PgPool, network_sid: i16) -> DataResult<Option<ClaimedChunk>> {
    let row: Option<(i64, i64, i64)> = sqlx::query_as(
        "UPDATE backfill_chunks
            SET status      = 'running',
                lease_until = now() + make_interval(secs => $2),
                updated_at  = now()
          WHERE id = (
              SELECT id FROM backfill_chunks
               WHERE network_id = $1
                 AND (status = 'pending'
                      OR (status = 'running'
                          AND (lease_until IS NULL OR lease_until < now())))
               ORDER BY id
               LIMIT 1
               FOR UPDATE SKIP LOCKED
          )
          RETURNING id, from_block, to_block",
    )
    .bind(network_sid)
    .bind(LEASE_DURATION.as_secs() as f64)
    .fetch_optional(pool)
    .await
    .map_err(|e| DataError::Rpc(format!("claim_chunk(net={network_sid}): {e}")))?;
    Ok(row.map(|(id, f, t)| ClaimedChunk {
        id,
        from_block: f as u64,
        to_block: t as u64,
    }))
}

/// Retire a successfully-indexed chunk. Clears the lease so nothing
/// else ever claims it again.
pub async fn mark_chunk_done(pool: &PgPool, id: i64) -> DataResult<()> {
    sqlx::query(
        "UPDATE backfill_chunks
            SET status      = 'done',
                lease_until = NULL,
                last_error  = NULL,
                updated_at  = now()
          WHERE id = $1",
    )
    .bind(id)
    .execute(pool)
    .await
    .map_err(|e| DataError::Rpc(format!("mark_chunk_done({id}): {e}")))?;
    Ok(())
}

/// Push a failed chunk back into `pending` so the next claim retries,
/// preserving the last error for audit. We deliberately don't go to a
/// permanent `failed` state: transient RPC / decode errors are the
/// common case and a silent retry is less surprising than a stuck
/// queue operators have to unblock by hand.
pub async fn mark_chunk_failed(pool: &PgPool, id: i64, err: &str) -> DataResult<()> {
    sqlx::query(
        "UPDATE backfill_chunks
            SET status      = 'pending',
                lease_until = NULL,
                last_error  = $2,
                updated_at  = now()
          WHERE id = $1",
    )
    .bind(id)
    .bind(truncate_error(err))
    .execute(pool)
    .await
    .map_err(|e| DataError::Rpc(format!("mark_chunk_failed({id}): {e}")))?;
    Ok(())
}

fn truncate_error(s: &str) -> String {
    if s.len() <= LAST_ERROR_CAP {
        s.to_string()
    } else {
        // Find the nearest char boundary ≤ LAST_ERROR_CAP so we never
        // split a multibyte codepoint and corrupt the text column.
        let mut end = LAST_ERROR_CAP;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

/// Count the chunks still waiting (`pending`) or in-flight (`running`)
/// for `network_id`. Exposed so the banner / tests can tell "backfill
/// is done" apart from "backfill never ran".
pub async fn open_chunk_count(pool: &PgPool, network_sid: i16) -> DataResult<u64> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM backfill_chunks
          WHERE network_id = $1 AND status IN ('pending','running')",
    )
    .bind(network_sid)
    .fetch_one(pool)
    .await
    .map_err(|e| DataError::Rpc(format!("open_chunk_count(net={network_sid}): {e}")))?;
    Ok(count.max(0) as u64)
}

/// Single backfill worker. Spawned by [`BackfillSupervisor::spawn`];
/// exposed publicly so integration tests can drive one directly against
/// a seeded queue without the supervisor machinery.
pub struct BackfillWorker {
    client: Arc<RpcClient>,
    pool: PgPool,
    id: usize,
    network_id: &'static str,
    network_sid: i16,
    author_lookup: Arc<super::lookups::AuthorLookup>,
}

impl BackfillWorker {
    pub fn new(
        network_id: &'static str,
        network_sid: i16,
        client: Arc<RpcClient>,
        pool: PgPool,
        id: usize,
        author_lookup: Arc<super::lookups::AuthorLookup>,
    ) -> Self {
        Self {
            client,
            pool,
            id,
            network_id,
            network_sid,
            author_lookup,
        }
    }

    /// Spawn on the current tokio runtime and return the handle. The
    /// worker logs + swallows its own errors — same contract as
    /// [`super::live::LiveWorker::spawn`] — so one crashed worker never
    /// blocks the others.
    pub fn spawn(self) -> JoinHandle<()> {
        tokio::spawn(async move {
            let label = self.id;
            let net = self.network_id;
            if let Err(e) = self.run().await {
                tracing::error!(
                    network = net,
                    worker = label,
                    error = %e,
                    "backfill worker exited with error"
                );
            }
        })
    }

    /// Worker loop. Never returns voluntarily — idle polls keep the
    /// worker alive so a late-seeded chunk or an expired lease is
    /// picked up without a restart.
    pub async fn run(self) -> DataResult<()> {
        // Prime the RPC supervisor so the first `at_block` doesn't pay
        // for the handshake on the critical path.
        self.client.subxt().await?;

        loop {
            match claim_chunk(&self.pool, self.network_sid).await? {
                Some(chunk) => {
                    tracing::debug!(
                        network = self.network_id,
                        worker = self.id,
                        chunk = chunk.id,
                        from = chunk.from_block,
                        to = chunk.to_block,
                        "backfill worker claimed chunk"
                    );
                    match self.index_chunk(&chunk).await {
                        Ok(()) => {
                            mark_chunk_done(&self.pool, chunk.id).await?;
                            tracing::debug!(
                                network = self.network_id,
                                worker = self.id,
                                chunk = chunk.id,
                                "backfill chunk done"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                network = self.network_id,
                                worker = self.id,
                                chunk = chunk.id,
                                error = %e,
                                "backfill chunk failed; releasing for retry"
                            );
                            mark_chunk_failed(&self.pool, chunk.id, &e.to_string()).await?;
                        }
                    }
                }
                None => {
                    tokio::time::sleep(IDLE_POLL).await;
                }
            }
        }
    }

    async fn index_chunk(&self, chunk: &ClaimedChunk) -> DataResult<()> {
        for num in chunk.from_block..=chunk.to_block {
            self.index_block(num).await?;
        }
        Ok(())
    }

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
        // Genesis snapshot walk — see [`super::live::LiveWorker::index_block`]
        // for the rationale. A backfill worker picking up block 0 after
        // the live path converges still replays idempotently via the
        // snapshot UPSERT's `last_activity_block`-guarded CASE.
        let genesis_snapshots = if num == 0 {
            projections::genesis::project_genesis_accounts(&at).await?
        } else {
            Vec::new()
        };

        let net = self.network_id;
        let sid = self.network_sid;
        let block_ts = row.timestamp_ms;

        let author_id = match row.author {
            Some(bytes) => Some(self.author_lookup.resolve(&self.pool, sid, bytes).await?),
            None => None,
        };

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| DataError::Rpc(format!("begin tx ({net}/backfill {num}): {e}")))?;
        sink::insert_block(&mut tx, sid, &row, author_id).await?;
        sink::insert_extrinsics(&mut tx, sid, block_ts, &extrinsic_rows).await?;
        sink::insert_events(&mut tx, sid, block_ts, &event_rows).await?;
        sink::insert_balance_movements(&mut tx, sid, &balance_projection.movements).await?;
        sink::apply_account_snapshots(&mut tx, sid, num, block_ts, &snapshots).await?;
        if num == 0 {
            sink::apply_account_snapshots(&mut tx, sid, 0, block_ts, &genesis_snapshots).await?;
        }
        sink::apply_ats(&mut tx, sid, &ats_ops).await?;
        tx.commit()
            .await
            .map_err(|e| DataError::Rpc(format!("commit tx ({net}/backfill {num}): {e}")))?;
        crate::server::metrics::record_block_indexed(net, crate::server::metrics::STREAM_BACKFILL);
        Ok(())
    }
}

/// Boot-side wiring: seed the chunk table once against the current
/// finalized head for this network, then spawn the worker pool. One
/// supervisor per indexed network. The seeding task and the workers
/// run concurrently — workers that race ahead just idle until the
/// seed writes land, which keeps the startup path free of a
/// potentially multi-second blocking query before the indexer comes
/// online.
pub struct BackfillSupervisor {
    client: Arc<RpcClient>,
    pool: PgPool,
    concurrency: usize,
    chunk_size: u64,
    network_id: &'static str,
    network_sid: i16,
    author_lookup: Arc<super::lookups::AuthorLookup>,
}

impl BackfillSupervisor {
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
            concurrency: DEFAULT_CONCURRENCY,
            chunk_size: DEFAULT_CHUNK_SIZE,
            network_id,
            network_sid,
            author_lookup,
        }
    }

    pub fn with_concurrency(mut self, c: usize) -> Self {
        self.concurrency = c.max(1);
        self
    }

    /// Spawn the seed task + the worker pool. Returns one handle per
    /// task; `main.rs` keeps them around to hold the workers alive for
    /// the process lifetime.
    pub fn spawn(self) -> Vec<JoinHandle<()>> {
        let Self {
            client,
            pool,
            concurrency,
            chunk_size,
            network_id,
            network_sid,
            author_lookup,
        } = self;

        let seed_client = client.clone();
        let seed_pool = pool.clone();
        let seed_handle: JoinHandle<()> = tokio::spawn(async move {
            if let Err(e) = seed_from_head(
                network_id,
                network_sid,
                &seed_client,
                &seed_pool,
                chunk_size,
            )
            .await
            {
                tracing::warn!(
                    network = network_id,
                    error = %e,
                    "backfill seed failed; workers will idle"
                );
            }
        });

        let mut handles = Vec::with_capacity(concurrency + 1);
        handles.push(seed_handle);
        for i in 0..concurrency {
            let w = BackfillWorker::new(
                network_id,
                network_sid,
                client.clone(),
                pool.clone(),
                i,
                author_lookup.clone(),
            );
            handles.push(w.spawn());
        }
        tracing::info!(
            network = network_id,
            concurrency,
            chunk_size,
            "backfill supervisor: workers spawned"
        );
        handles
    }
}

/// Wait up to [`SEED_HEAD_GRACE`] for the finalized-head watch to fire
/// once, then compute + INSERT the chunk set for `network_id`. If the
/// head never fires (chain frozen, disconnected node) we log and
/// return `Ok(())` — an empty queue leaves workers idle, the live
/// path keeps serving.
async fn seed_from_head(
    network_id: &str,
    network_sid: i16,
    client: &RpcClient,
    pool: &PgPool,
    chunk_size: u64,
) -> DataResult<()> {
    client.subxt().await?;
    let mut rx = client.watch_finalized_head();

    if rx.borrow().is_none() {
        let _ = tokio::time::timeout(SEED_HEAD_GRACE, async {
            loop {
                if rx.changed().await.is_err() {
                    return;
                }
                if rx.borrow().is_some() {
                    return;
                }
            }
        })
        .await;
    }

    let Some(head) = *rx.borrow() else {
        tracing::warn!(
            network = network_id,
            grace = ?SEED_HEAD_GRACE,
            "backfill seed: no finalized head observed, skipping"
        );
        return Ok(());
    };

    let inserted = seed_chunks(pool, network_sid, head, chunk_size).await?;
    tracing::info!(
        network = network_id,
        head,
        inserted,
        "backfill seed complete"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cold DB → one chunk every `chunk_size` blocks, inclusive on both
    /// ends. This is the "ceil(N / chunk_size) chunks" simplification
    /// the plan calls out for empty-DB boot.
    #[test]
    fn full_gap_cold_db() {
        let chunks = split_gaps_into_chunks(999, &[], 100);
        assert_eq!(
            chunks.len(),
            10,
            "999 / 100 rounded up = 10 chunks, not {}",
            chunks.len()
        );
        assert_eq!(chunks.first(), Some(&(0, 99)));
        assert_eq!(chunks.last(), Some(&(900, 999)));
        // Chunks must tile the range without holes or overlap.
        for w in chunks.windows(2) {
            assert_eq!(w[0].1 + 1, w[1].0, "chunks must be contiguous");
        }
    }

    /// Partially-indexed DB: the prefix `[0, 999]` already landed in a
    /// prior run, the rest of the range is still missing. Exactly two
    /// chunks: the full [1000, 1999] and the trailing [2000, 2500].
    #[test]
    fn partial_gap_existing_prefix() {
        let existing: Vec<u64> = (0..=999).collect();
        let chunks = split_gaps_into_chunks(2500, &existing, 1000);
        assert_eq!(chunks, vec![(1000, 1999), (2000, 2500)]);
    }

    /// DB is caught up to the head — no chunks to seed, the worker pool
    /// has nothing to do. This is the steady-state after backfill drains.
    #[test]
    fn no_gap() {
        let existing: Vec<u64> = (0..=9).collect();
        let chunks = split_gaps_into_chunks(9, &existing, 1000);
        assert!(
            chunks.is_empty(),
            "no missing blocks should yield no chunks, got {chunks:?}"
        );
    }

    /// Chunk-size boundary: `[0, 999]` fits exactly into one chunk, and
    /// the very next block `[1000]` opens a second one of width 1.
    /// Locks the "inclusive on both sides, width = chunk_size" contract.
    #[test]
    fn boundary_at_chunk_size() {
        let chunks = split_gaps_into_chunks(999, &[], 1000);
        assert_eq!(
            chunks,
            vec![(0, 999)],
            "exactly chunk_size blocks = one chunk"
        );

        let chunks = split_gaps_into_chunks(1000, &[], 1000);
        assert_eq!(
            chunks,
            vec![(0, 999), (1000, 1000)],
            "chunk_size + 1 blocks = two chunks, the second one block wide"
        );
    }

    /// A hole in the middle of the indexed range produces a single
    /// chunk covering just the missing window. The adjacent indexed
    /// rows don't contaminate the range.
    #[test]
    fn gap_in_middle_splits_into_single_range() {
        let existing: Vec<u64> = (0..=9).chain(30..=40).collect();
        let chunks = split_gaps_into_chunks(40, &existing, 1000);
        assert_eq!(chunks, vec![(10, 29)]);
    }

    /// Defensive: entries out of `[0, head]` or regressive w.r.t. the
    /// running cursor are skipped. Bad input shouldn't crash the boot
    /// path — the contract is "caller feeds sorted", but the function
    /// tolerates noise.
    #[test]
    fn out_of_range_entries_are_skipped() {
        let existing: Vec<u64> = vec![0, 1, 2, 200, 300]; // 200/300 > head
        let chunks = split_gaps_into_chunks(10, &existing, 100);
        assert_eq!(chunks, vec![(3, 10)]);
    }

    /// `chunk_size = 1` is the degenerate case: one chunk per block.
    /// Useful for tests that want to exercise the claim loop with many
    /// small units of work.
    #[test]
    fn chunk_size_one_splits_each_block() {
        let chunks = split_gaps_into_chunks(3, &[], 1);
        assert_eq!(chunks, vec![(0, 0), (1, 1), (2, 2), (3, 3)]);
    }
}
