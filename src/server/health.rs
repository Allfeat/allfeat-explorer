//! Health signals consumed by orchestrators and the UI banner.
//!
//! ## Kubernetes probes
//!
//! `/healthz` answers "is the process alive enough to accept traffic" — a
//! plain 200 once the HTTP stack is up. Kube's liveness probe never needs
//! more than that; tying it to upstream state would make transient RPC
//! hiccups trigger a pod restart, which is the opposite of what we want.
//!
//! `/readyz` answers "should the LB route traffic to this pod right now"
//! and layers two checks:
//!
//! 1. `ChainData::is_ready` — at least one upstream node has produced a
//!    finalized head. RPC boot takes a couple of seconds; during that
//!    window the response is 503 and any upstream LB skips the pod.
//! 2. When a Postgres pool is configured, the live cursor's age must
//!    stay under [`READY_MAX_LAG_SECONDS`]. A stuck live worker
//!    otherwise keeps serving stale data silently — flipping the pod
//!    out of rotation surfaces the incident to whatever monitors the
//!    LB.
//!
//! The decision logic is extracted to a pure [`ready_verdict`] helper so
//! integration tests cover every branch without spinning up an axum
//! server — they just inject the resolved inputs and assert the verdict.
//!
//! ## Indexer status (banner)
//!
//! [`IndexerStatus`] is the per-network snapshot consumed by the
//! `IndexingBanner` in the Nuxt frontend. [`compute_status`] is the pure
//! derivation from chain head / DB cursor / backfill progress;
//! [`collect_status_from`] composes it with live sqlx reads. The HTTP
//! handler exposing `Vec<IndexerStatus>` at `/api/v1/indexing/status`
//! lives in the API router (Phase 2) and consumes these helpers.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ts-bindings")]
use ts_rs::TS;

use super::AppState;

/// Lag budget before `/readyz` flips to 503. The live worker bumps its
/// cursor every finalized block (3–6 s on Allfeat networks), so 30 s
/// is roughly ten block intervals — a generous margin that avoids
/// readiness flapping on a single slow commit while still catching a
/// genuinely wedged worker before alerting humans.
pub const READY_MAX_LAG_SECONDS: i64 = 30;

/// Outcome of [`ready_verdict`]. Kept as an enum (not a bool) so the
/// response body can carry the specific failure reason — operators
/// tailing the probe logs get an actionable message instead of a
/// generic "not ready".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadyVerdict {
    Ok,
    BackendNotReady,
    DbUnreachable,
    CursorStale(i64),
}

/// Pure decision function. `backend_ready` is the
/// `ChainData::is_ready` readout; `cursor_age` is `Some` when the DB
/// is configured and the cursor row exists, `None` otherwise; `db_error`
/// is true when the cursor query itself failed.
///
/// Evaluation order matters: a missing backend is strictly more severe
/// than a stuck indexer (no chain = nothing to serve), and a DB error
/// is strictly more severe than a stale cursor (we can't even tell
/// how stale it is). Report the most-severe signal so the 503 body is
/// actionable.
pub fn ready_verdict(
    backend_ready: bool,
    cursor_age: Option<i64>,
    db_error: bool,
    max_lag_seconds: i64,
) -> ReadyVerdict {
    if !backend_ready {
        return ReadyVerdict::BackendNotReady;
    }
    if db_error {
        return ReadyVerdict::DbUnreachable;
    }
    if let Some(age) = cursor_age {
        if age > max_lag_seconds {
            return ReadyVerdict::CursorStale(age);
        }
    }
    ReadyVerdict::Ok
}

/// Liveness probe. Always 200 — if the process can serve this route it's
/// alive, by definition.
pub async fn healthz() -> impl IntoResponse {
    StatusCode::OK
}

/// Readiness probe. 200 when the backend is ready **and** every
/// indexed network has a fresh cursor; 503 otherwise with a body
/// explaining which check failed.
///
/// The per-network check reduces to the **worst** verdict: one stuck
/// chain flips the whole pod out of rotation. Pages for the healthy
/// chains keep working via the RPC fallback, but we'd rather surface
/// the incident at the LB layer than serve silently-stale data.
pub async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let backend_ready = state.provider.is_ready();

    // Resolve the DB-side inputs across every indexed network. In mock
    // builds the indexer never runs so the pair collapses to `(None,
    // false)` and the verdict falls through on `backend_ready` alone.
    #[cfg(not(feature = "mock"))]
    let (cursor_age, db_error) = match (state.db_pool.as_ref(), state.network_lookup.get()) {
        (Some(pool), Some(lookup)) => {
            use crate::indexer::sink;
            let mut worst_age: Option<i64> = None;
            let mut saw_error = false;
            for network_id in state.indexed_network_ids.iter().copied() {
                let Some(sid) = lookup.resolve(network_id) else {
                    continue;
                };
                match sink::cursor_age_seconds(pool, sid, sink::LIVE_CURSOR).await {
                    Ok(Some(age)) => {
                        worst_age = Some(worst_age.map_or(age, |prev| prev.max(age)));
                    }
                    Ok(None) => {}
                    Err(_) => saw_error = true,
                }
            }
            (worst_age, saw_error)
        }
        _ => (None, false),
    };
    #[cfg(feature = "mock")]
    let (cursor_age, db_error): (Option<i64>, bool) = (None, false);

    let verdict = ready_verdict(backend_ready, cursor_age, db_error, READY_MAX_LAG_SECONDS);
    verdict_response(verdict)
}

fn verdict_response(v: ReadyVerdict) -> axum::response::Response {
    match v {
        ReadyVerdict::Ok => (StatusCode::OK, "ready".to_string()).into_response(),
        ReadyVerdict::BackendNotReady => (
            StatusCode::SERVICE_UNAVAILABLE,
            "no ready backend".to_string(),
        )
            .into_response(),
        ReadyVerdict::DbUnreachable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "indexer db unreachable".to_string(),
        )
            .into_response(),
        ReadyVerdict::CursorStale(age) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("indexer stale: live cursor age {age}s > {READY_MAX_LAG_SECONDS}s"),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_not_ready_wins_over_everything() {
        // Even if the DB-side signal looks fine, no chain → no service.
        // Locking the priority here keeps readyz from hiding an
        // upstream outage behind an indexer-only error line.
        let v = ready_verdict(false, Some(0), false, 30);
        assert_eq!(v, ReadyVerdict::BackendNotReady);
    }

    #[test]
    fn db_error_beats_cursor_age() {
        // When the cursor query errored we don't know the real age —
        // reporting `CursorStale(Option)` would be fiction. Surface
        // the more specific "db unreachable" verdict instead.
        let v = ready_verdict(true, None, true, 30);
        assert_eq!(v, ReadyVerdict::DbUnreachable);
    }

    #[test]
    fn fresh_cursor_is_ok() {
        let v = ready_verdict(true, Some(5), false, 30);
        assert_eq!(v, ReadyVerdict::Ok);
    }

    #[test]
    fn stale_cursor_flips_503() {
        // Strict `>` — 30s exactly is still OK (boundary slack for
        // the "last block took the full interval" case).
        let boundary = ready_verdict(true, Some(30), false, 30);
        assert_eq!(boundary, ReadyVerdict::Ok);
        let over = ready_verdict(true, Some(31), false, 30);
        assert_eq!(over, ReadyVerdict::CursorStale(31));
    }

    #[test]
    fn missing_cursor_is_ok_during_boot() {
        // `None` age = cursor row not yet written (fresh DB, first
        // block still indexing). Treating this as 503 would stall the
        // pod forever on a cold deploy — accept it until the first
        // commit lands.
        let v = ready_verdict(true, None, false, 30);
        assert_eq!(v, ReadyVerdict::Ok);
    }
}

// ---------------------------------------------------------------------------
// Indexer status — per-network snapshot consumed by the UI banner.
// ---------------------------------------------------------------------------

/// Snapshot of one network's indexer pose, consumed by the banner.
/// All optional fields stay `None` when a reading isn't available yet
/// (cold boot, no DB configured) so the UI can fall back to a
/// "healthy" rendering without guessing.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IndexerStatus {
    /// Network this status describes — matches `NetworkSpec::id`.
    pub network_id: String,
    pub state: IndexerState,
    /// Latest finalized block number observed on the chain. `None`
    /// before the finalized watcher has fired once.
    #[serde(with = "crate::serde_helpers::u64_string_opt")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string | null"))]
    pub finalized_head: Option<u64>,
    /// Latest block the indexer has persisted for this network.
    #[serde(with = "crate::serde_helpers::u64_string_opt")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string | null"))]
    pub indexer_head: Option<u64>,
    /// `finalized_head - indexer_head` when both are known.
    #[serde(with = "crate::serde_helpers::u64_string_opt")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string | null"))]
    pub live_lag_blocks: Option<u64>,
    /// Number of finalized blocks already indexed.
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub backfill_done: u64,
    /// Expected total once backfill completes.
    #[serde(with = "crate::serde_helpers::u64_string")]
    #[cfg_attr(feature = "ts-bindings", ts(type = "string"))]
    pub backfill_total: u64,
    /// `backfill_done / backfill_total * 100`, clamped to `[0, 100]`.
    pub backfill_pct: f32,
}

/// Traffic-light states used by the banner. Ordered from best to
/// worst so downstream comparisons (e.g. "any state worse than
/// CatchingUp?") stay obvious.
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexerState {
    /// Lag ≤ 2 blocks AND `backfill_pct = 100`. Banner row stays hidden.
    Healthy,
    /// Lag > 2 blocks but indexer cursor keeps moving.
    CatchingUp,
    /// Historical backfill still running (`backfill_pct < 100`).
    Backfilling,
    /// Cursor stale > 60 s or DB unreachable — indexer effectively down.
    Offline,
}

impl IndexerStatus {
    /// Pre-indexer default for `network_id`: nothing known, nothing to
    /// show. Returned when no DB is wired or the context isn't available.
    pub fn healthy_stub(network_id: impl Into<String>) -> Self {
        Self {
            network_id: network_id.into(),
            state: IndexerState::Healthy,
            finalized_head: None,
            indexer_head: None,
            live_lag_blocks: None,
            backfill_done: 0,
            backfill_total: 0,
            backfill_pct: 100.0,
        }
    }
}

/// Lag threshold that flips a network row from `Healthy` to
/// `CatchingUp`.
pub const HEALTHY_LAG_BLOCKS: u64 = 2;

/// Cursor age over which the indexer is considered `Offline`.
pub const OFFLINE_CURSOR_AGE_SECONDS: i64 = 60;

/// Pure derivation of one network's banner state.
pub fn compute_status(
    network_id: impl Into<String>,
    finalized_head: Option<u64>,
    indexer_head: Option<u64>,
    cursor_age_seconds: Option<i64>,
    backfill_done: u64,
    backfill_total: u64,
) -> IndexerStatus {
    let live_lag_blocks = match (finalized_head, indexer_head) {
        (Some(f), Some(i)) => Some(f.saturating_sub(i)),
        _ => None,
    };

    let backfill_pct = if backfill_total == 0 {
        100.0
    } else {
        ((backfill_done as f64 / backfill_total as f64) * 100.0).clamp(0.0, 100.0) as f32
    };

    let state = derive_state(live_lag_blocks, cursor_age_seconds, backfill_pct);

    IndexerStatus {
        network_id: network_id.into(),
        state,
        finalized_head,
        indexer_head,
        live_lag_blocks,
        backfill_done,
        backfill_total,
        backfill_pct,
    }
}

/// Banner decision tree. Order matters: a stuck cursor always wins
/// over a lag readout. Backfill wins over lag — during a multi-hour
/// backfill, the live-lag would flap wildly.
fn derive_state(
    live_lag_blocks: Option<u64>,
    cursor_age_seconds: Option<i64>,
    backfill_pct: f32,
) -> IndexerState {
    if let Some(age) = cursor_age_seconds {
        if age > OFFLINE_CURSOR_AGE_SECONDS {
            return IndexerState::Offline;
        }
    }
    if backfill_pct < 100.0 {
        return IndexerState::Backfilling;
    }
    if let Some(lag) = live_lag_blocks {
        if lag > HEALTHY_LAG_BLOCKS {
            return IndexerState::CatchingUp;
        }
    }
    IndexerState::Healthy
}

/// Composition of the banner status for `network_id`: takes the
/// chain-side finalized head and the (optional) DB pool, reads the
/// rest off Postgres.
pub async fn collect_status_from(
    network_id: &'static str,
    #[cfg_attr(feature = "mock", allow(unused_variables))] network_sid: i16,
    finalized_head: Option<u64>,
    pool: Option<&sqlx::PgPool>,
) -> IndexerStatus {
    let (indexer_head, cursor_age, backfill_done) = match pool {
        #[cfg(not(feature = "mock"))]
        Some(pool) => {
            use crate::indexer::sink;
            let head = sink::load_cursor(pool, network_sid, sink::LIVE_CURSOR)
                .await
                .ok()
                .flatten();
            let age = sink::cursor_age_seconds(pool, network_sid, sink::LIVE_CURSOR)
                .await
                .ok()
                .flatten();
            let done = count_indexed_blocks(pool, network_sid).await.unwrap_or(0);
            (head, age, done)
        }
        #[cfg(feature = "mock")]
        Some(_) => (None, None, 0),
        None => (None, None, 0),
    };

    let backfill_total = finalized_head.map(|h| h.saturating_add(1)).unwrap_or(0);

    compute_status(
        network_id,
        finalized_head,
        indexer_head,
        cursor_age,
        backfill_done,
        backfill_total,
    )
}

/// Gather the status of every indexed network from the live
/// [`AppState`]. Order matches `AppState::indexed_network_ids`. Never
/// hits chain RPC on this path — the banner refreshes every few
/// seconds and must stay cache-friendly.
pub async fn collect_indexer_status(state: &AppState) -> Vec<IndexerStatus> {
    let mut out = Vec::with_capacity(state.indexed_network_ids.len());
    for network_id in state.indexed_network_ids.iter().copied() {
        #[cfg(not(feature = "mock"))]
        let finalized_head = state
            .indexer_clients
            .get(network_id)
            .and_then(|c| c.finalized_head());
        #[cfg(feature = "mock")]
        let finalized_head: Option<u64> = None;

        #[cfg(not(feature = "mock"))]
        let network_sid = state
            .network_lookup
            .get()
            .and_then(|l| l.resolve(network_id))
            .unwrap_or(0);
        #[cfg(feature = "mock")]
        let network_sid: i16 = 0;

        out.push(
            collect_status_from(
                network_id,
                network_sid,
                finalized_head,
                state.db_pool.as_ref(),
            )
            .await,
        );
    }
    out
}

/// Row count of `blocks` for `network_sid`.
#[cfg(not(feature = "mock"))]
async fn count_indexed_blocks(pool: &sqlx::PgPool, network_sid: i16) -> Result<u64, sqlx::Error> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM blocks WHERE network_id = $1")
        .bind(network_sid)
        .fetch_one(pool)
        .await?;
    Ok(n.max(0) as u64)
}

#[cfg(test)]
mod indexer_status_tests {
    use super::*;

    #[test]
    fn healthy_when_everything_is_quiet() {
        let s = compute_status("allfeat", Some(100), Some(100), Some(2), 0, 0);
        assert_eq!(s.state, IndexerState::Healthy);
        assert_eq!(s.live_lag_blocks, Some(0));
        assert!((s.backfill_pct - 100.0).abs() < f32::EPSILON);
    }

    #[test]
    fn small_lag_still_healthy() {
        let s = compute_status("allfeat", Some(102), Some(100), Some(1), 0, 0);
        assert_eq!(s.state, IndexerState::Healthy);
        assert_eq!(s.live_lag_blocks, Some(2));
    }

    #[test]
    fn catching_up_when_lag_exceeds_threshold() {
        let s = compute_status("allfeat", Some(500), Some(100), Some(1), 0, 0);
        assert_eq!(s.state, IndexerState::CatchingUp);
        assert_eq!(s.live_lag_blocks, Some(400));
    }

    #[test]
    fn offline_when_cursor_is_stale() {
        let s = compute_status("allfeat", Some(500), Some(100), Some(120), 0, 0);
        assert_eq!(s.state, IndexerState::Offline);
    }

    #[test]
    fn backfilling_when_pct_under_hundred() {
        let s = compute_status("allfeat", Some(500), Some(100), Some(1), 50, 100);
        assert_eq!(s.state, IndexerState::Backfilling);
        assert!((s.backfill_pct - 50.0).abs() < 0.01);
    }

    #[test]
    fn healthy_stub_is_well_formed() {
        let s = IndexerStatus::healthy_stub("allfeat");
        assert_eq!(s.network_id, "allfeat");
        assert_eq!(s.state, IndexerState::Healthy);
        assert!(s.finalized_head.is_none());
        assert!(s.indexer_head.is_none());
        assert_eq!(s.backfill_total, 0);
    }

    #[test]
    fn missing_heads_collapse_to_no_lag_reading() {
        let s = compute_status("allfeat", None, Some(100), Some(1), 0, 0);
        assert_eq!(s.state, IndexerState::Healthy);
        assert!(s.live_lag_blocks.is_none());
    }
}
