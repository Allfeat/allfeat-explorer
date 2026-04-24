//! Per-route `Cache-Control` policy helpers for the REST API.
//!
//! Four tiers mapped onto the endpoint categories:
//!
//! * [`LATEST`] — lists and head-following feeds that roll forward every
//!   block (`/blocks`, `/transfers`, `/events`, `/waveform`, `/ats/feed`,
//!   top accounts, account-by-address). Short `max-age` matched to the
//!   hot moka TTL so browser + backend expire in lockstep, generous SWR
//!   so revisits feel instant.
//! * [`DETAIL`] — `(block|extrinsic|ats|envelope)`-by-id responses.
//!   Stable once the referenced block finalises; `max-age` rides out the
//!   GRANDPA window (~30 s) and SWR covers a day of revisits.
//! * [`STATIC`] — deploy/upgrade-scoped responses (runtime blobs,
//!   metadata, network list, build info). Runtime upgrades ship on a
//!   scale of days to weeks, so minutes of browser cache are safe.
//! * [`NO_STORE`] — `/indexing/status`. A stale lag reading would mask a
//!   stuck indexer from the UI banner.
//!
//! Headers are only applied to 2xx responses (except [`NO_STORE`], which
//! also needs to cover errors so a cached 500 can't pretend the indexer
//! is fine). A 404 on `/blocks/:n` often means "chain hasn't reached
//! that height yet"; caching it would lock the user out of the block
//! for the TTL window.

use axum::extract::Request;
use axum::http::{header, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

/// Listings + head-following feeds. `max-age=2` matches
/// `crate::data::rpc::cache::HOT_TTL_SECS`.
pub const LATEST: &str = "public, max-age=2, stale-while-revalidate=60";

/// Detail-by-id on indexed records.
pub const DETAIL: &str = "public, max-age=15, stale-while-revalidate=86400";

/// Deploy-bounded state: runtime, metadata, network list, build info.
pub const STATIC: &str = "public, max-age=300, stale-while-revalidate=3600";

/// Indexer status — must never be cached.
pub const NO_STORE: &str = "no-store";

pub async fn latest(req: Request, next: Next) -> Response {
    apply(LATEST, next.run(req).await, true)
}

pub async fn detail(req: Request, next: Next) -> Response {
    apply(DETAIL, next.run(req).await, true)
}

pub async fn static_(req: Request, next: Next) -> Response {
    apply(STATIC, next.run(req).await, true)
}

pub async fn no_store(req: Request, next: Next) -> Response {
    apply(NO_STORE, next.run(req).await, false)
}

fn apply(policy: &'static str, mut response: Response, success_only: bool) -> Response {
    if success_only && !response.status().is_success() {
        return response;
    }
    if !response.headers().contains_key(header::CACHE_CONTROL) {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(policy));
    }
    response
}
