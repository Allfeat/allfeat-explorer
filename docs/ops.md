# Operator runbook — indexer & DB

Companion to `docs/deployment.md`. That document covers boot-time config
and network exposure; this one covers the everyday "something is off with
Postgres / the indexer, what do I touch" questions.

All commands assume the operator is on a shell that has `DATABASE_URL`
set to the production connection string, and `psql` / `pg_dump` /
`pg_restore` on `$PATH`. Substitute the network / host names as needed.

## Health signals

| Signal                          | Source                                        | Meaning                                              |
|---------------------------------|-----------------------------------------------|------------------------------------------------------|
| `GET /healthz`                  | HTTP                                          | Process is alive. 200 always — never gates restarts. |
| `GET /readyz`                   | HTTP                                          | 200 when RPC + indexer are both fresh. 503 body names the failed check. |
| `GET /metrics`                  | HTTP                                          | Prometheus scrape. §"Key metrics" below.             |
| Banner at the top of every page | `get_indexer_status` server fn                | Same data as `/metrics` for non-ops users.           |
| `SELECT * FROM indexer_cursor;` | Postgres                                      | Last block indexed per stream. Updated every commit. |

The banner and `/readyz` agree by design: both check the same lag budget
(`READY_MAX_LAG_SECONDS = 30`, see `src/server/health.rs`). If they
disagree in prod, something is caching one of them — suspect a stale
browser tab or a proxy buffering `/readyz` responses.

## Key metrics

| Series                                | Type    | Alert suggestion                                |
|---------------------------------------|---------|-------------------------------------------------|
| `indexer_blocks_indexed_total{stream}`| counter | `rate(...[5m]) == 0` for 5 min = stuck worker   |
| `indexer_lag_seconds`                 | gauge   | `> 30` for 2 min = matches `/readyz` threshold  |
| `indexer_lag_blocks`                  | gauge   | `> 60` for 5 min = falling behind the chain     |
| `indexer_backfill_remaining_blocks`   | gauge   | Trending flat for 30 min during backfill = jam  |
| `indexer_reorg_total`                 | counter | Any increase before the reorg-aware buffer ships → unexpected, investigate |
| `buffer_size` / `buffer_best_head`    | gauge   | Size pegged at the cap (64) = finalization stalled upstream |

All indexer series are primed with zero at boot (`register_descriptions`
in `src/server/metrics.rs`), so absence from `/metrics` indicates a build
without the `ssr` feature rather than a silenced counter.

## Backup & restore

### Baseline daily backup

```bash
pg_dump --format=custom --no-owner --no-privileges \
    --file=explorer-$(date -u +%Y%m%dT%H%M%SZ).dump \
    "$DATABASE_URL"
```

Keep 14 daily snapshots. The `custom` format is compressed and
`pg_restore`-friendly without going through `psql`.

### Restore onto a fresh instance

```bash
createdb explorer
pg_restore --no-owner --no-privileges --dbname=explorer explorer-YYYYMMDDTHHMMSSZ.dump
```

Applying `migrations/` first is **not** required — the dump already
carries the schema. If the dump is older than the current code's
migration set, the next backend start applies the delta automatically
(the binary runs `MIGRATOR.run(&pool)` at boot when `DATABASE_URL` is
set). To force the migration step without serving traffic, run the
binary in `--mode=indexer` against the restored DB and stop it once the
`migrations: schema up to date` log line appears.

### Point-in-time recovery

Out of scope for this document. Use Postgres WAL archiving + your
host's PITR tooling; the application has no extra requirements beyond
a consistent snapshot of every table.

## Resetting the backfill

The backfill queue is idempotent — chunks are `ON CONFLICT DO NOTHING`
on insert and `indexer_cursor` uses `GREATEST` on every bump, so rerunning
anything is safe. Two common operations:

### "I want to force a re-index of a specific range"

Delete the block rows and re-seed the chunks for the right network.
`ON DELETE CASCADE` on `extrinsics` / `events` / `balance_movements`
handles the cleanup.

```sql
-- Wipe the derived data for a range on ONE network. Account balances
-- are NOT rewound — they're cumulative and can't be undone by
-- deleting rows.
DELETE FROM blocks
 WHERE network_id = 'allfeat' AND num BETWEEN 123456 AND 124456;

-- Queue the range for the backfill workers on that network.
INSERT INTO backfill_chunks (network_id, from_block, to_block, status)
VALUES ('allfeat', 123456, 124456, 'pending');
```

The running `indexer` / `all` process picks the chunk up on the next
poll cycle (≤ 2 s) — no restart needed.

### "I want to wipe everything and re-backfill from genesis"

Wipes every network. Most of the time you want the per-network
variant below instead.

```sql
TRUNCATE blocks, backfill_chunks, indexer_cursor,
         account_balances, ats_registry, ats_versions
         RESTART IDENTITY CASCADE;
```

`CASCADE` handles `extrinsics` / `events` / `balance_movements` via
their FK. Restart the indexer process — it re-seeds
`backfill_chunks` at the first finalized-head tick for every
indexed network.

### "I want to wipe ONE network and re-backfill just that chain"

```sql
BEGIN;
DELETE FROM blocks            WHERE network_id = 'melodie';
DELETE FROM backfill_chunks   WHERE network_id = 'melodie';
DELETE FROM indexer_cursor    WHERE network_id = 'melodie';
DELETE FROM account_balances  WHERE network_id = 'melodie';
DELETE FROM ats_registry      WHERE network_id = 'melodie';
DELETE FROM runtime_versions  WHERE network_id = 'melodie';
COMMIT;
```

Extrinsics / events / balance_movements / ats_versions cascade from
`blocks` and `ats_registry`. The running `indexer` / `all` process
re-seeds the other network's chunks automatically on the next
finalized-head tick.

## Rebuilding `account_balances`

`account_balances` holds absolute `System::Account` snapshots, one
row per `(network_id, account)`, refreshed by every block that
touches the account. Replaying is idempotent — the same block reads
the same chain state and writes the same values — so a stale row
converges on its own the next time that account moves. If you need
to force convergence (e.g. after a logic change to the touched-set
collector), wipe and re-index:

```sql
-- One network:
DELETE FROM account_balances WHERE network_id = 'allfeat';
-- Or all networks:
TRUNCATE account_balances;
```

Because the snapshot UPSERT only fires when an account is touched,
inactive rows won't rebuild from a simple `DELETE` + wait. Use
`--reconcile` for the companion sweep:

```bash
./allfeat-explorer --reconcile
```

The sweep walks every row in `account_balances` (per indexed
network), refetches each account's state from `System::Account` at
the current head, and UPDATEs free/reserved/frozen/nonce in place.
`first_seen_block` and `last_activity_block` are deliberately not
touched — reconciliation is maintenance, not activity.

Exit codes:

| code | meaning                                                            |
|------|--------------------------------------------------------------------|
| 0    | every configured network converged                                 |
| 2    | `--reconcile` set but `DATABASE_URL` unset                         |
| 4    | `--reconcile` set but no indexed networks configured               |
| 5    | at least one network's sweep failed (see logs)                     |

Best run with the indexer stopped: the live pipeline's per-block
UPSERT and the sweep's UPDATE both target `account_balances`, and a
concurrent live write at an older block can briefly regress a
just-reconciled row until the next touch. The race is self-healing
(the next block that touches the account re-writes the correct
value) but strict determinism wants a maintenance window.

## Deployment modes

| Mode          | DB required | HTTP  | Notes                                                     |
|---------------|-------------|-------|-----------------------------------------------------------|
| `--mode=all`  | optional    | yes   | Dev + onboarding. Without `DATABASE_URL` serves via RPC.  |
| `--mode=indexer` | yes      | no    | Worker-only replica. Boot refuses without `DATABASE_URL`. |
| `--mode=server`  | yes      | yes   | HTTP-only replica. Boot refuses without `DATABASE_URL`.   |

Prod topology is "1 indexer + N servers" reading the same Postgres.
The indexer writes every declared network into the shared DB —
multi-tenant via a `network_id` column on every indexed table. Each
`server` replica still needs one `RPC_ENDPOINT_<NETWORK>` variable
per network because it runs its own best-head subscription for each
chain's in-RAM buffer — the finalized path comes from Postgres, but
the tip can't.

`--mode=indexer` doesn't serve `/metrics` (no HTTP stack). Observe it
through Postgres (`indexer_cursor.updated_at`) and structured logs
instead. Every other mode exports the full metric set.

## Known limitations

* **No retention policy.** `events` and `balance_movements` grow
  monotonically. When either crosses ~50 M rows (~4 M blocks on
  Melodie), revisit §9 of `docs/indexing-plan.md` for the partitioning
  plan.
* **`indexer_reorg_total` is always 0 today.** The best-block buffer
  only appends; reorg-aware handling is deferred to a later phase.
  The series is exported anyway so the alerting rule can sit idle
  until the counter starts moving.
* **Balances are finalized-only.** An account page never shows a
  "pending balance" overlay from the buffer — by design, see
  `docs/indexing-plan.md` §"Règles de partage des responsabilités".
