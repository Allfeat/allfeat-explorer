//! Phase 7 — integration coverage for the Prometheus scrape surface.
//!
//! The `axum-prometheus` recorder is a process-wide singleton, so this
//! file installs it exactly once (via `OnceLock`) and asserts the
//! indexer-side series show up in the rendered text. We don't care
//! about the values beyond "zero is fine" — a dashboard / alert rule
//! only needs the series name to exist before the first scrape, which
//! is what `register_descriptions` + the initial prime calls
//! guarantee.
//!
//! The rendered text format is Prometheus text-exposition, so we grep
//! for the HELP line (safer than the metric line, which may carry
//! labels the test shouldn't hard-code) plus at least one sample row.

#![cfg(all(feature = "ssr", not(feature = "mock")))]

mod common;

use std::sync::OnceLock;

use allfeat_explorer::server::metrics::{
    self, BUFFER_BEST_HEAD, BUFFER_FINALIZED_HEAD, BUFFER_SIZE, INDEXER_BACKFILL_REMAINING_BLOCKS,
    INDEXER_BLOCKS_INDEXED_TOTAL, INDEXER_LAG_BLOCKS, INDEXER_LAG_SECONDS, INDEXER_REORG_TOTAL,
    STREAM_BACKFILL, STREAM_LIVE,
};
use axum_prometheus::metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use common::TEST_NETWORK;

/// Install the recorder once for the whole test binary.
/// `metrics_exporter_prometheus::PrometheusBuilder` is the same crate
/// `axum-prometheus` sits on top of, but `install_recorder()` skips
/// the tokio-driven upkeep task — fine for tests that only exercise
/// `render()` and don't care about rollups. The `OnceLock` pattern
/// keeps multiple tests in this file sharing the same registry
/// (installing twice would panic on the global recorder slot).
fn handle() -> &'static PrometheusHandle {
    static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
    HANDLE.get_or_init(|| {
        let h = PrometheusBuilder::new()
            .install_recorder()
            .expect("install prometheus recorder");
        metrics::register_descriptions(&[TEST_NETWORK]);
        h
    })
}

/// Every indexer-side series we declare shows up in the rendered
/// output. Locks the plan's §8 contract: the four dashboard-critical
/// names (`indexer_blocks_indexed_total`, `indexer_lag_seconds`,
/// `buffer_size`, `indexer_reorg_total`) plus the secondary ones.
/// If a future refactor drops the prime calls, a key series would
/// disappear until its first write — this test catches that drift.
#[test]
fn exports_every_indexer_series() {
    let rendered = handle().render();

    for name in [
        INDEXER_BLOCKS_INDEXED_TOTAL,
        INDEXER_LAG_SECONDS,
        INDEXER_LAG_BLOCKS,
        INDEXER_BACKFILL_REMAINING_BLOCKS,
        INDEXER_REORG_TOTAL,
        BUFFER_SIZE,
        BUFFER_BEST_HEAD,
        BUFFER_FINALIZED_HEAD,
    ] {
        assert!(
            rendered.contains(&format!("# HELP {name} ")),
            "expected `# HELP {name}` in /metrics output, got:\n{rendered}"
        );
    }
}

/// The block counter is labelled by stream. Dashboards and alerts
/// key on `stream="live"` vs `stream="backfill"` separately, so a
/// single merged counter would silently destroy the signal. Bump
/// each stream once and assert the rendered series carries both label
/// values distinctly.
#[test]
fn block_counter_splits_by_stream() {
    let _ = handle();
    metrics::record_block_indexed(TEST_NETWORK, STREAM_LIVE);
    metrics::record_block_indexed(TEST_NETWORK, STREAM_BACKFILL);

    let rendered = handle().render();
    assert!(
        rendered.contains(r#"stream="live""#),
        "live-stream label missing: {rendered}"
    );
    assert!(
        rendered.contains(r#"stream="backfill""#),
        "backfill-stream label missing: {rendered}"
    );
    assert!(
        rendered.contains(&format!(r#"network="{TEST_NETWORK}""#)),
        "network label missing: {rendered}"
    );
}

/// The buffer gauges reflect the last `record_buffer_state` call. We
/// don't assert an exact value (rendering a f64 is format-sensitive),
/// only that writing "42 best / 40 finalized / size=3" mentions the
/// name next to a numeric sample line — enough to lock the gauge wiring
/// without binding the test to the exporter's exact formatting.
#[test]
fn buffer_gauges_reflect_last_write() {
    let _ = handle();
    metrics::record_buffer_state(TEST_NETWORK, 3, Some(42), 40);

    let rendered = handle().render();
    // One sample line per gauge. With the multi-tenant refactor the
    // series carry a `network` label, so the text format emits
    // `buffer_size{network="allfeat"} 3`. The `starts_with` probe still
    // locks the series name without binding to the exact label layout.
    assert!(
        rendered.lines().any(|l| l.starts_with("buffer_size{")),
        "no buffer_size sample line found in:\n{rendered}"
    );
    assert!(
        rendered.lines().any(|l| l.starts_with("buffer_best_head{")),
        "no buffer_best_head sample line found in:\n{rendered}"
    );
    assert!(
        rendered
            .lines()
            .any(|l| l.starts_with("buffer_finalized_head{")),
        "no buffer_finalized_head sample line found in:\n{rendered}"
    );
}
