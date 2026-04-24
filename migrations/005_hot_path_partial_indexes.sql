-- Hot-path partial indexes.
--
-- Slow-query log flagged `latest_extrinsics` at ~1.2 s for LIMIT 26 on a
-- table with >1 M rows. The existing PK `(network_id, block_num, idx)`
-- already matches the ORDER BY, but the default filter
--
--     NOT (e.idx = 0 AND e.signer IS NULL)
--
-- strips the per-block timestamp inherent (always at idx=0, always
-- unsigned) and requires a heap-tuple probe per candidate to check
-- `signer IS NULL` — the signer column isn't in the PK index. With
-- ~6 inherent rows skipped per 26 matches, a LIMIT 26 query scans
-- O(block_count × avg_extrinsics_per_block) heap pages before filling
-- the page. Moving the filter into the index materialises the
-- non-inherent slice directly; Postgres walks the tip of the index
-- newest-first, never touches the heap until it has exactly `count`
-- matches, and returns in microseconds.
--
-- Same pattern applies to `list_transfers_page` / `latest_transfers`:
-- they need the `kind = 0 AND delta < 0` slice of balance_movements
-- (the "sender side of a transfer" — half the Transfer rows). The
-- generic `balance_movements_kind_block_idx` added in 004 covers
-- `(kind, block_num DESC)` but still needs a heap probe to check the
-- sign of `delta`. A narrower partial index drops the sign check and
-- is roughly half the size.

-- ── extrinsics: non-timestamp-inherent slice ─────────────────────────────

CREATE INDEX extrinsics_nonts_idx
  ON extrinsics (network_id, block_num DESC, idx DESC)
  WHERE NOT (idx = 0 AND signer IS NULL);

-- ── balance_movements: transfer-out (sender side) slice ──────────────────

DROP INDEX IF EXISTS balance_movements_kind_block_idx;

CREATE INDEX balance_movements_transfer_out_idx
  ON balance_movements (network_id, block_num DESC, event_idx DESC)
  WHERE kind = 0 AND delta < 0;
