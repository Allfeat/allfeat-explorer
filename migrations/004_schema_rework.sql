-- Schema rework — storage + read-path optimisation.
--
-- Destructive migration: drops every application table (blocks,
-- extrinsics, events, balance_movements, account_balances, ats_*) and
-- every admin table that carries network_id (runtime_versions,
-- indexer_cursor, backfill_chunks), then recreates the whole set with
-- a smaller per-row footprint and denormalised timestamps. Rerunning
-- the live + backfill workers repopulates everything from RPC; no
-- application-owned row carries state that isn't also on the chain.
--
-- What changes vs. 001-003
--
--   1. `network_id TEXT` → `network_id SMALLINT` everywhere. A single
--      row carrying a 9-byte string on 50M+ rows weighs ~450 MB in
--      raw columns and bloats every composite index by the same
--      prefix. The network lookup below is populated once at startup
--      from the compile-time network ids and never changes shape.
--   2. `blocks.author BYTEA (32 bytes)` → `blocks.author_id INT` FK
--      into `authors`. ~15-20 distinct validators per chain, repeated
--      across millions of blocks — the old layout stored the same
--      32-byte key once per block. FK keeps the lookup unique per
--      network (the same AccountId32 on two chains is a different
--      validator, so (network_id, bytes) stays composite).
--   3. Dead columns dropped: `extrinsics.error_module`,
--      `extrinsics.error_name`. They were written by the projection
--      but never surfaced (destructured as `_error_module` /
--      `_error_name` in src/data/indexed/queries/extrinsics.rs), so
--      removing them is lossless.
--   4. Denormalised `timestamp_ms` onto `extrinsics` and `events` so
--      the list/detail reads don't have to JOIN `blocks` just to
--      render wall-clock. Costs 8 bytes/row; buys a full JOIN cut on
--      every hot read (transfers go 4-way → 3-way, extrinsic list
--      goes 2-way → 1-way).
--   5. Denormalised `first_seen_ms` / `last_activity_ms` onto
--      `account_balances` for the same reason — the old
--      `account_by_address` / `top_accounts` paths did TWO LEFT JOINs
--      to `blocks` per row just to translate block numbers into
--      wall clock.
--   6. New index `balance_movements_kind_block_idx` backs the
--      transfer list's `kind = 0 AND delta < 0` filter (the old
--      `account_idx` couldn't help that slice).
--   7. `blocks_timestamp_idx` dropped — nothing ORDERs BY
--      `timestamp_ms`, the PK covers the block-num path already.
--
-- All FKs are declared ON DELETE CASCADE so a `DELETE FROM blocks`
-- reorg surgery cleans up extrinsics/events/balance_movements the
-- same way 001 did. `authors` rows intentionally don't cascade — a
-- validator is long-lived; we don't want a block reorg to free the
-- id underneath a replay of the same block.

-- ── Tear-down ─────────────────────────────────────────────────────────────

DROP TABLE IF EXISTS
    balance_movements,
    account_balances,
    ats_versions,
    ats_registry,
    events,
    extrinsics,
    blocks,
    runtime_versions,
    indexer_cursor,
    backfill_chunks
  CASCADE;

-- ── Lookup tables ─────────────────────────────────────────────────────────

-- `id` is provided by the application (from a compile-time constant per
-- network) so the value is stable across environments — not a SERIAL
-- that reorders rows if dev DBs are seeded in a different sequence.
-- Kept SMALLINT because the expected count stays well under 100.
CREATE TABLE networks (
    id   SMALLINT PRIMARY KEY,
    name TEXT     NOT NULL UNIQUE
);

-- One row per unique block author ever observed, scoped per network.
-- SERIAL because the ids are internal — callers resolve them through
-- the in-process `AuthorLookup` cache and never round-trip the value
-- back out to humans.
CREATE TABLE authors (
    id         SERIAL   PRIMARY KEY,
    network_id SMALLINT NOT NULL REFERENCES networks(id),
    bytes      BYTEA    NOT NULL,
    UNIQUE (network_id, bytes)
);

-- ── Chain state ───────────────────────────────────────────────────────────

CREATE TABLE blocks (
    network_id      SMALLINT NOT NULL REFERENCES networks(id),
    num             BIGINT   NOT NULL,
    hash            BYTEA    NOT NULL,
    parent_hash     BYTEA    NOT NULL,
    state_root      BYTEA    NOT NULL,
    extrinsics_root BYTEA    NOT NULL,
    author_id       INT               REFERENCES authors(id),
    timestamp_ms    BIGINT   NOT NULL,
    spec_version    INT      NOT NULL,
    extrinsic_count INT      NOT NULL,
    event_count     INT      NOT NULL,
    ref_time        BIGINT   NOT NULL DEFAULT 0,
    proof_size      BIGINT   NOT NULL DEFAULT 0,
    ref_time_pct    SMALLINT NOT NULL DEFAULT 0,
    size_bytes      INT      NOT NULL DEFAULT 0,
    indexed_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (network_id, num)
);
CREATE UNIQUE INDEX blocks_hash_idx   ON blocks (network_id, hash);
CREATE INDEX        blocks_author_idx ON blocks (network_id, author_id, num DESC)
    WHERE author_id IS NOT NULL;

CREATE TABLE extrinsics (
    network_id   SMALLINT    NOT NULL,
    block_num    BIGINT      NOT NULL,
    idx          INT         NOT NULL,
    hash         BYTEA       NOT NULL,
    pallet       TEXT        NOT NULL,
    call         TEXT        NOT NULL,
    signer       BYTEA,
    tip          NUMERIC(39),
    fee          NUMERIC(39) NOT NULL,
    nonce        BIGINT,
    success      BOOLEAN     NOT NULL,
    args_scale   BYTEA       NOT NULL,
    timestamp_ms BIGINT      NOT NULL,
    PRIMARY KEY (network_id, block_num, idx),
    FOREIGN KEY (network_id, block_num) REFERENCES blocks(network_id, num) ON DELETE CASCADE
);
CREATE INDEX extrinsics_signer_idx      ON extrinsics (network_id, signer, block_num DESC, idx)
    WHERE signer IS NOT NULL;
CREATE INDEX extrinsics_hash_idx        ON extrinsics (network_id, hash);
CREATE INDEX extrinsics_pallet_call_idx ON extrinsics (network_id, pallet, call, block_num DESC);

CREATE TABLE events (
    network_id   SMALLINT NOT NULL,
    block_num    BIGINT   NOT NULL,
    idx          INT      NOT NULL,
    phase_kind   SMALLINT NOT NULL,
    phase_idx    INT,
    pallet       TEXT     NOT NULL,
    variant      TEXT     NOT NULL,
    data_scale   BYTEA    NOT NULL,
    timestamp_ms BIGINT   NOT NULL,
    PRIMARY KEY (network_id, block_num, idx),
    FOREIGN KEY (network_id, block_num) REFERENCES blocks(network_id, num) ON DELETE CASCADE
);
CREATE INDEX events_pallet_variant_idx ON events (network_id, pallet, variant, block_num DESC);
CREATE INDEX events_phase_idx          ON events (network_id, block_num, phase_kind, phase_idx);

-- ── Balances ──────────────────────────────────────────────────────────────

CREATE TABLE balance_movements (
    network_id   SMALLINT    NOT NULL,
    block_num    BIGINT      NOT NULL,
    event_idx    INT         NOT NULL,
    account      BYTEA       NOT NULL,
    kind         SMALLINT    NOT NULL,
    delta        NUMERIC(39) NOT NULL,
    counterparty BYTEA,
    PRIMARY KEY (network_id, block_num, event_idx, account),
    FOREIGN KEY (network_id, block_num) REFERENCES blocks(network_id, num) ON DELETE CASCADE
);
CREATE INDEX balance_movements_account_idx
    ON balance_movements (network_id, account, block_num DESC, event_idx);
-- New in 004: transfer listing filters on `(kind = 0 AND delta < 0)`,
-- which the account_idx doesn't cover. Composite leads with the
-- filter column so the planner can range-scan the slice directly.
CREATE INDEX balance_movements_kind_block_idx
    ON balance_movements (network_id, kind, block_num DESC, event_idx DESC);

CREATE TABLE account_balances (
    network_id          SMALLINT    NOT NULL REFERENCES networks(id),
    account             BYTEA       NOT NULL,
    free                NUMERIC(39) NOT NULL DEFAULT 0,
    reserved            NUMERIC(39) NOT NULL DEFAULT 0,
    frozen              NUMERIC(39) NOT NULL DEFAULT 0,
    nonce               BIGINT      NOT NULL DEFAULT 0,
    first_seen_block    BIGINT      NOT NULL,
    last_activity_block BIGINT      NOT NULL,
    first_seen_ms       BIGINT      NOT NULL DEFAULT 0,
    last_activity_ms    BIGINT      NOT NULL DEFAULT 0,
    PRIMARY KEY (network_id, account)
);
CREATE INDEX account_balances_total_idx ON account_balances (network_id, (free + reserved) DESC);

-- ── ATS ────────────────────────────────────────────────────────────────────

CREATE TABLE ats_registry (
    network_id      SMALLINT NOT NULL REFERENCES networks(id),
    id              BIGINT   NOT NULL,
    owner           BYTEA    NOT NULL,
    created_block   BIGINT   NOT NULL,
    created_ext_idx INT      NOT NULL,
    version_count   INT      NOT NULL DEFAULT 0,
    PRIMARY KEY (network_id, id)
);
CREATE INDEX ats_registry_owner_idx ON ats_registry (network_id, owner, id DESC);

CREATE TABLE ats_versions (
    network_id       SMALLINT NOT NULL,
    ats_id           BIGINT   NOT NULL,
    version          INT      NOT NULL,
    block_num        BIGINT   NOT NULL,
    ext_idx          INT      NOT NULL,
    metadata_cid     TEXT,
    commitment       BYTEA,
    protocol_version SMALLINT,
    PRIMARY KEY (network_id, ats_id, version),
    FOREIGN KEY (network_id, ats_id) REFERENCES ats_registry(network_id, id) ON DELETE CASCADE
);
CREATE INDEX ats_versions_block_idx ON ats_versions (network_id, block_num DESC, ats_id);

-- ── Runtime metadata ledger ───────────────────────────────────────────────

CREATE TABLE runtime_versions (
    network_id       SMALLINT NOT NULL REFERENCES networks(id),
    spec_version     INT      NOT NULL,
    first_seen_block BIGINT   NOT NULL,
    metadata_blob    BYTEA    NOT NULL,
    PRIMARY KEY (network_id, spec_version)
);

-- ── Indexer state ─────────────────────────────────────────────────────────

CREATE TABLE indexer_cursor (
    network_id   SMALLINT    NOT NULL REFERENCES networks(id),
    stream       TEXT        NOT NULL,
    last_indexed BIGINT      NOT NULL,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (network_id, stream)
);

CREATE TABLE backfill_chunks (
    id          BIGSERIAL   PRIMARY KEY,
    network_id  SMALLINT    NOT NULL REFERENCES networks(id),
    from_block  BIGINT      NOT NULL,
    to_block    BIGINT      NOT NULL,
    status      TEXT        NOT NULL,
    lease_until TIMESTAMPTZ,
    last_error  TEXT,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (network_id, from_block, to_block)
);
CREATE INDEX backfill_chunks_status_idx ON backfill_chunks (network_id, status, lease_until);
