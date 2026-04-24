//! `MetadataCache` — memoises runtime metadata per `spec_version` so
//! projections don't re-fetch it for every historic block. Implemented
//! alongside the backfill worker in Phase 2 of `docs/indexing-plan.md`
//! (the live worker only sees one `spec_version` per runtime upgrade,
//! which subxt already caches behind `OnlineClient`).
