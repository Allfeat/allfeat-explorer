-- Initial schema for the Allfeat Explorer indexer.
--
-- Scope: Phase 0 of docs/indexing-plan.md. Every table and index listed in
-- §1 is created here in a single migration — later phases start filling
-- these tables but the schema stays frozen until the first partitioning
-- pass.
--
-- Conventions:
--   * BYTEA for every chain hash / AccountId32 — smaller than hex strings
--     and indexes more compactly.
--   * NUMERIC(39, 0) for balances / fees / tips — u128 max fits in 39
--     digits, gives us native +/-/ORDER BY without cast gymnastics.
--   * Composite PKs on the "per block" tables (blocks are the natural
--     unit; no BIGSERIAL in disguise).
--   * Indexes target the hot paths: pagination (`ORDER BY … DESC`),
--     search by hash / signer / pallet-call, account history joins.
--   * `network_id TEXT` on every indexed table: one deployment indexes
--     several chains into the same DB. Every PK/UNIQUE and every index
--     leads with `network_id` so Postgres uses it for filter *and*
--     ordering (see `docs/indexing-plan.md` §9.3).

-- ── Chain state ───────────────────────────────────────────────────────────

CREATE TABLE blocks (
  network_id      TEXT   NOT NULL,
  num             BIGINT NOT NULL,
  hash            BYTEA  NOT NULL,
  parent_hash     BYTEA  NOT NULL,
  state_root      BYTEA  NOT NULL,
  extrinsics_root BYTEA  NOT NULL,
  author          BYTEA,
  timestamp_ms    BIGINT NOT NULL,
  spec_version    INT    NOT NULL,
  extrinsic_count INT    NOT NULL,
  event_count     INT    NOT NULL,
  indexed_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (network_id, num)
);
-- Hash is only unique *within* a network: two chains can theoretically
-- collide on genesis blob or trivial blocks. Named explicitly so ops
-- tooling can reference `blocks_hash_idx` by name.
CREATE UNIQUE INDEX blocks_hash_idx      ON blocks (network_id, hash);
CREATE INDEX        blocks_author_idx    ON blocks (network_id, author, num DESC) WHERE author IS NOT NULL;
CREATE INDEX        blocks_timestamp_idx ON blocks (network_id, timestamp_ms DESC);

CREATE TABLE extrinsics (
  network_id   TEXT         NOT NULL,
  block_num    BIGINT       NOT NULL,
  idx          INT          NOT NULL,
  hash         BYTEA        NOT NULL,
  pallet       TEXT         NOT NULL,
  call         TEXT         NOT NULL,
  signer       BYTEA,
  tip          NUMERIC(39),
  fee          NUMERIC(39)  NOT NULL,
  nonce        BIGINT,
  success      BOOLEAN      NOT NULL,
  error_module TEXT,
  error_name   TEXT,
  args_scale   BYTEA        NOT NULL,
  PRIMARY KEY (network_id, block_num, idx),
  FOREIGN KEY (network_id, block_num) REFERENCES blocks(network_id, num) ON DELETE CASCADE
);
CREATE INDEX extrinsics_signer_idx      ON extrinsics (network_id, signer, block_num DESC, idx) WHERE signer IS NOT NULL;
CREATE INDEX extrinsics_hash_idx        ON extrinsics (network_id, hash);
CREATE INDEX extrinsics_pallet_call_idx ON extrinsics (network_id, pallet, call, block_num DESC);

CREATE TABLE events (
  network_id  TEXT     NOT NULL,
  block_num   BIGINT   NOT NULL,
  idx         INT      NOT NULL,
  phase_kind  SMALLINT NOT NULL,        -- 0=ApplyExtrinsic, 1=Finalization, 2=Initialization
  phase_idx   INT,                       -- extrinsic idx if phase_kind=0, NULL otherwise
  pallet      TEXT     NOT NULL,
  variant     TEXT     NOT NULL,
  data_scale  BYTEA    NOT NULL,
  PRIMARY KEY (network_id, block_num, idx),
  FOREIGN KEY (network_id, block_num) REFERENCES blocks(network_id, num) ON DELETE CASCADE
);
CREATE INDEX events_pallet_variant_idx ON events (network_id, pallet, variant, block_num DESC);
CREATE INDEX events_phase_idx          ON events (network_id, block_num, phase_kind, phase_idx);

-- ── Balances ──────────────────────────────────────────────────────────────

CREATE TABLE balance_movements (
  network_id   TEXT         NOT NULL,
  block_num    BIGINT       NOT NULL,
  event_idx    INT          NOT NULL,
  account      BYTEA        NOT NULL,
  kind         SMALLINT     NOT NULL,   -- MovementKind enum (see below)
  delta        NUMERIC(39)  NOT NULL,   -- signed; NUMERIC supports negatives
  counterparty BYTEA,                    -- Transfer only — the other account
  PRIMARY KEY (network_id, block_num, event_idx, account),
  FOREIGN KEY (network_id, block_num) REFERENCES blocks(network_id, num) ON DELETE CASCADE
);
CREATE INDEX balance_movements_account_idx ON balance_movements (network_id, account, block_num DESC, event_idx);

-- MovementKind (SMALLINT enum — source of truth lives in Rust; keep
-- this comment in lockstep with `indexer::projections::balances::MovementKind`):
--   0  Transfer (p2p — two rows, one per account, opposite signs)
--   1  Deposit
--   2  Withdraw
--   3  Fee                 (TransactionPayment.TransactionFeePaid)
--   4  Slash
--   5  Reserve
--   6  Unreserve
--   7  ReserveRepatriated
--   8  Burn
--   9  Minted
--  10  Frozen
--  11  Thawed
--  12  Locked
--  13  Unlocked
--  14  Held                (fungible::hold — amount added to reserved)
--  15  Released            (fungible::hold release — amount subtracted)
--  16  BurnedHeld          (held balance burned — amount subtracted)
--  17  TransferAndHold     (two rows, source -> dest, held at dest)
--  18  TransferOnHold      (two rows, held funds moved source -> dest)
--  19  Endowed             (new account with `free_balance`)
--  20  DustLost            (account reaped below existential deposit)
--  21  Restored            (previously suspended amount returned)
--  22  Suspended           (amount suspended from an account)

CREATE TABLE account_balances (
  network_id          TEXT         NOT NULL,
  account             BYTEA        NOT NULL,
  free                NUMERIC(39)  NOT NULL DEFAULT 0,
  reserved            NUMERIC(39)  NOT NULL DEFAULT 0,
  frozen              NUMERIC(39)  NOT NULL DEFAULT 0,
  nonce               BIGINT       NOT NULL DEFAULT 0,
  first_seen_block    BIGINT       NOT NULL,
  last_activity_block BIGINT       NOT NULL,
  PRIMARY KEY (network_id, account)
);
CREATE INDEX account_balances_total_idx ON account_balances (network_id, (free + reserved) DESC);

-- ── ATS (Allfeat-specific) ────────────────────────────────────────────────

CREATE TABLE ats_registry (
  network_id      TEXT   NOT NULL,
  id              BIGINT NOT NULL,
  owner           BYTEA  NOT NULL,
  created_block   BIGINT NOT NULL,
  created_ext_idx INT    NOT NULL,
  version_count   INT    NOT NULL DEFAULT 0,
  PRIMARY KEY (network_id, id)
);
CREATE INDEX ats_registry_owner_idx ON ats_registry (network_id, owner, id DESC);

CREATE TABLE ats_versions (
  network_id   TEXT   NOT NULL,
  ats_id       BIGINT NOT NULL,
  version      INT    NOT NULL,
  block_num    BIGINT NOT NULL,
  ext_idx      INT    NOT NULL,
  metadata_cid TEXT,
  PRIMARY KEY (network_id, ats_id, version),
  FOREIGN KEY (network_id, ats_id) REFERENCES ats_registry(network_id, id) ON DELETE CASCADE
);
CREATE INDEX ats_versions_block_idx ON ats_versions (network_id, block_num DESC, ats_id);

-- ── Runtime metadata ledger ───────────────────────────────────────────────
--
-- spec_version is per-chain — two networks can collide on the same number.

CREATE TABLE runtime_versions (
  network_id       TEXT   NOT NULL,
  spec_version     INT    NOT NULL,
  first_seen_block BIGINT NOT NULL,
  metadata_blob    BYTEA  NOT NULL,
  PRIMARY KEY (network_id, spec_version)
);

-- ── Indexer state ─────────────────────────────────────────────────────────
--
-- One row per (network, stream). `stream` is still 'live' | 'backfill'
-- but now scoped per network so the two chains progress independently.

CREATE TABLE indexer_cursor (
  network_id   TEXT        NOT NULL,
  stream       TEXT        NOT NULL,   -- 'live' | 'backfill' | …
  last_indexed BIGINT      NOT NULL,
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (network_id, stream)
);

CREATE TABLE backfill_chunks (
  id          BIGSERIAL   PRIMARY KEY,
  network_id  TEXT        NOT NULL,
  from_block  BIGINT      NOT NULL,
  to_block    BIGINT      NOT NULL,
  status      TEXT        NOT NULL,       -- pending | running | done | failed
  lease_until TIMESTAMPTZ,                -- recovery after a worker crash
  last_error  TEXT,
  updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (network_id, from_block, to_block)
);
CREATE INDEX backfill_chunks_status_idx ON backfill_chunks (network_id, status, lease_until);
