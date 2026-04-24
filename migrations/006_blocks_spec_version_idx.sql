-- Covering index for the runtime-upgrade timeline.
--
-- `runtime_upgrades` (src/data/indexed/queries/runtime.rs) runs
--
--     SELECT DISTINCT ON (spec_version) spec_version, num, timestamp_ms
--     FROM blocks WHERE network_id = $1
--     ORDER BY spec_version, num ASC
--
-- on every hit of the runtime-history endpoint. The existing indexes on
-- `blocks` lead with `num` / `hash` / `author_id` — none match the
-- `(spec_version, num ASC)` sort, so Postgres seq-scans the whole
-- per-network slice and sorts just to emit ~3 rows. Slow-query log
-- flagged it at 1.27 s once the chain crossed ~1 M indexed blocks.
--
-- `INCLUDE (timestamp_ms)` turns the plan into an index-only scan: the
-- three output columns are all resolvable from the index, no heap
-- probe. Writes pay one extra btree insert per block, which is cheap
-- compared to the other five indexes already on `blocks`.
--
-- The longer-term plan (see `runtime.rs` header comment) is to fill the
-- `runtime_versions` table from the indexer and drop this aggregate
-- entirely; this migration is the tide-over until that lands.

CREATE INDEX blocks_spec_version_num_idx
  ON blocks (network_id, spec_version, num)
  INCLUDE (timestamp_ms);
