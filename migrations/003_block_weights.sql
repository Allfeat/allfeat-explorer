-- Persist per-block weight + size so the indexed `block_by_number` /
-- `latest_blocks` paths surface the same `ref_time` / `proof_size` /
-- `size_bytes` the RPC mapper computes. The Phase 1 schema dropped these
-- because they weren't on the critical path; the block detail page reads
-- them from the DB once the indexer catches up, so a finalised block was
-- showing zeros after the row landed in `blocks` (cf. row_to_block in
-- src/data/indexed/queries/blocks.rs).
--
-- BIGINT for the weights — `u64` ref_time/proof_size in the runtime, but
-- a real block stays well under `i64::MAX`. INT for size_bytes (block
-- size << 2 GB). SMALLINT for ref_time_pct (0..=100, computed at index
-- time against the block's own runtime constant so a future runtime
-- upgrade doesn't reinterpret old rows).
--
-- Default 0 keeps existing rows valid; the columns will be properly
-- populated for every block indexed after this migration runs.
-- Re-index to backfill historical rows.

ALTER TABLE blocks
  ADD COLUMN ref_time     BIGINT   NOT NULL DEFAULT 0,
  ADD COLUMN proof_size   BIGINT   NOT NULL DEFAULT 0,
  ADD COLUMN ref_time_pct SMALLINT NOT NULL DEFAULT 0,
  ADD COLUMN size_bytes   INTEGER  NOT NULL DEFAULT 0;
