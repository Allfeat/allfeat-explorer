-- Phase 6 — ATS projection columns.
--
-- The Phase 1 schema (`001_initial.sql`) left `ats_versions` with just
-- `metadata_cid TEXT` for the per-version payload, which matched the
-- plan's v1 sketch but pre-dated the concrete pallet-ats event shape.
-- The on-chain events actually carry a 32-byte commitment hash plus a
-- 1-byte protocol version — neither of those round-trips through TEXT
-- without losing the binary shape or adding a decode step on every
-- read. Store both natively.
--
-- Nullable on purpose: rows already landed from earlier ad-hoc Phase 6
-- work (if any test DB survived the migration) stay valid; new Phase 6
-- inserts populate both columns.

ALTER TABLE ats_versions
  ADD COLUMN commitment       BYTEA,
  ADD COLUMN protocol_version SMALLINT;
