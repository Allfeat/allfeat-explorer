//! Indexer-side Prometheus counters / gauges.
//!
//! The HTTP request-layer metrics ship for free via `axum-prometheus`
//! (`axum_http_requests_total`, latency histograms, …). This module
//! layers the **indexer-specific** series on top through the `metrics`
//! facade so dashboards can cover the projection pipeline, the
//! best-block buffer, and the lag signals that gate `/readyz`.
//!
//! **Multi-tenant labels.** Every indexer series carries a `network`
//! label — one deployment indexes several chains into the same
//! Postgres, so dashboards need to slice by chain. The `stream` label
//! on the blocks counter stays alongside `network`.
//!
//! Conventions:
//!
//! * Counters are monotonically increasing; the value a scrape observes
//!   is the running total since the process started.
//! * Gauges can move in both directions. A refresher task (see
//!   [`spawn_pose_refresher`]) keeps the derived ones honest even when
//!   the indexer sits idle.
//! * Every metric is emitted with an initial zero during
//!   [`register_descriptions`] so the series appears in the first
//!   scrape, before any real write lands.

use std::time::Duration;

use metrics::{counter, describe_counter, describe_gauge, gauge};
use sqlx::PgPool;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Blocks persisted by the indexer, labelled by `(network, stream)`.
pub const INDEXER_BLOCKS_INDEXED_TOTAL: &str = "indexer_blocks_indexed_total";
/// Seconds since the live cursor last advanced (labelled by `network`).
pub const INDEXER_LAG_SECONDS: &str = "indexer_lag_seconds";
/// `finalized_head - indexer_head`, in blocks (labelled by `network`).
pub const INDEXER_LAG_BLOCKS: &str = "indexer_lag_blocks";
/// Finalized blocks still missing from `blocks` per network.
pub const INDEXER_BACKFILL_REMAINING_BLOCKS: &str = "indexer_backfill_remaining_blocks";
/// Reorgs observed by the best-block buffer (labelled by `network`).
/// Currently always 0 — the buffer only appends — but we publish the
/// series so the alert rule is already wired the day reorg handling
/// lands.
pub const INDEXER_REORG_TOTAL: &str = "indexer_reorg_total";
/// Blocks currently held in the pending buffer (labelled by `network`).
pub const BUFFER_SIZE: &str = "buffer_size";
/// Highest block number in the pending buffer (labelled by `network`).
pub const BUFFER_BEST_HEAD: &str = "buffer_best_head";
/// Finalized head the buffer has advanced past (labelled by `network`).
pub const BUFFER_FINALIZED_HEAD: &str = "buffer_finalized_head";

/// Stream label values for [`INDEXER_BLOCKS_INDEXED_TOTAL`].
pub const STREAM_LIVE: &str = "live";
pub const STREAM_BACKFILL: &str = "backfill";

/// How often the pose refresher recomputes the lag gauges. Matches the
/// banner poll cadence — fast enough that a stuck cursor flips the
/// gauges within a scrape window, slow enough that a scrape during a
/// quiet indexer never pays for more than one COUNT(*) on `blocks`.
pub const POSE_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// Register descriptions AND prime every metric with a zero-valued
/// sample so the series shows up in the first `/metrics` scrape even
/// before the indexer has moved. Safe to call more than once.
///
/// Pass the set of indexed networks so each per-network series lands
/// in the exporter pre-primed — otherwise the series only appears
/// after the first increment.
pub fn register_descriptions(networks: &[&'static str]) {
    describe_counter!(
        INDEXER_BLOCKS_INDEXED_TOTAL,
        "Blocks persisted by the indexer, labelled by (network, stream)."
    );
    describe_gauge!(
        INDEXER_LAG_SECONDS,
        "Seconds since the live cursor last advanced. /readyz flips 503 past the lag threshold."
    );
    describe_gauge!(
        INDEXER_LAG_BLOCKS,
        "finalized_head - indexer_head, in blocks. 0 when the live worker is caught up."
    );
    describe_gauge!(
        INDEXER_BACKFILL_REMAINING_BLOCKS,
        "Finalized blocks still missing from the blocks table."
    );
    describe_counter!(
        INDEXER_REORG_TOTAL,
        "Reorgs observed by the best-block buffer."
    );
    describe_gauge!(BUFFER_SIZE, "Blocks currently held in the pending buffer.");
    describe_gauge!(
        BUFFER_BEST_HEAD,
        "Highest block number in the pending buffer (0 when empty)."
    );
    describe_gauge!(
        BUFFER_FINALIZED_HEAD,
        "Finalized head the pending buffer has advanced past."
    );

    // Prime the series per-network. `counter!(..).increment(0)` is the
    // idiomatic "register but don't move the running total" nudge.
    for net in networks {
        counter!(INDEXER_BLOCKS_INDEXED_TOTAL, "network" => *net, "stream" => STREAM_LIVE)
            .increment(0);
        counter!(INDEXER_BLOCKS_INDEXED_TOTAL, "network" => *net, "stream" => STREAM_BACKFILL)
            .increment(0);
        counter!(INDEXER_REORG_TOTAL, "network" => *net).increment(0);
        gauge!(INDEXER_LAG_SECONDS, "network" => *net).set(0.0);
        gauge!(INDEXER_LAG_BLOCKS, "network" => *net).set(0.0);
        gauge!(INDEXER_BACKFILL_REMAINING_BLOCKS, "network" => *net).set(0.0);
        gauge!(BUFFER_SIZE, "network" => *net).set(0.0);
        gauge!(BUFFER_BEST_HEAD, "network" => *net).set(0.0);
        gauge!(BUFFER_FINALIZED_HEAD, "network" => *net).set(0.0);
    }
}

/// Bump the per-stream indexed-block counter for `network`.
pub fn record_block_indexed(network: &'static str, stream: &'static str) {
    counter!(INDEXER_BLOCKS_INDEXED_TOTAL, "network" => network, "stream" => stream).increment(1);
}

/// Snapshot the current buffer sizing for `network`.
pub fn record_buffer_state(
    network: &'static str,
    size: usize,
    best_head: Option<u64>,
    finalized_head: u64,
) {
    gauge!(BUFFER_SIZE, "network" => network).set(size as f64);
    gauge!(BUFFER_BEST_HEAD, "network" => network).set(best_head.unwrap_or(0) as f64);
    gauge!(BUFFER_FINALIZED_HEAD, "network" => network).set(finalized_head as f64);
}

/// Publish the derived indexer lag signals for `network`.
pub fn record_indexer_pose(
    network: &'static str,
    lag_seconds: f64,
    lag_blocks: u64,
    backfill_remaining: u64,
) {
    gauge!(INDEXER_LAG_SECONDS, "network" => network).set(lag_seconds);
    gauge!(INDEXER_LAG_BLOCKS, "network" => network).set(lag_blocks as f64);
    gauge!(INDEXER_BACKFILL_REMAINING_BLOCKS, "network" => network).set(backfill_remaining as f64);
}

/// Bump the reorg counter for `network`. Stub today — wired for the
/// future reorg-aware buffer.
pub fn record_reorg(network: &'static str) {
    counter!(INDEXER_REORG_TOTAL, "network" => network).increment(1);
}

/// Spawn a per-network background task that periodically recomputes
/// lag + backfill-remaining gauges off the DB cursor and the network's
/// finalized-head watch.
///
/// `interval` defaults to [`POSE_REFRESH_INTERVAL`]; the knob is
/// public so integration tests can tick faster without waiting on the
/// production cadence.
pub fn spawn_pose_refresher(
    network_id: &'static str,
    network_sid: i16,
    pool: PgPool,
    finalized_head_rx: watch::Receiver<Option<u64>>,
    interval: Duration,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            refresh_pose_once(network_id, network_sid, &pool, &finalized_head_rx).await;
        }
    })
}

/// Single refresh cycle for `network_id`, extracted so tests can drive
/// the pose computation deterministically without standing up the
/// timer task.
pub async fn refresh_pose_once(
    network_id: &'static str,
    network_sid: i16,
    pool: &PgPool,
    finalized_head_rx: &watch::Receiver<Option<u64>>,
) {
    let finalized_head = *finalized_head_rx.borrow();
    let status = crate::server::health::collect_status_from(
        network_id,
        network_sid,
        finalized_head,
        Some(pool),
    )
    .await;
    let age = crate::indexer::sink::cursor_age_seconds(
        pool,
        network_sid,
        crate::indexer::sink::LIVE_CURSOR,
    )
    .await
    .ok()
    .flatten()
    .unwrap_or(0);
    let lag_blocks = status.live_lag_blocks.unwrap_or(0);
    let backfill_remaining = status.backfill_total.saturating_sub(status.backfill_done);
    record_indexer_pose(network_id, age as f64, lag_blocks, backfill_remaining);
}
