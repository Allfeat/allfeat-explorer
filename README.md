# Allfeat Explorer

Block explorer for the [Allfeat](https://github.com/Allfeat/allfeat) mainnet
and the Melodie testnet. Two processes:

- **Rust backend** (Axum + `subxt` + `sqlx`) — REST at `/api/v1/*` and a
  multiplexed WebSocket at `/api/v1/live`, listening on `:8088`.
- **Nuxt 4 frontend** (Vue 3 SSR, Bun toolchain) — serves the UI on `:3000`
  and proxies `/api/*` to the backend.

Chain data reaches the UI through a single `ChainData` provider trait, wired
to one of three implementations at compile time: a Postgres-backed indexed
provider (prod default when `DATABASE_URL` is set), a `subxt` RPC provider
(fallback), or a deterministic in-memory mock (dev / e2e).

## Quick start

The root `package.json` exposes the loops via Bun:

```bash
# Mock backend (no node, no DB) + Nuxt dev server.
bun run dev:api        # Rust, --features ssr,mock, :8088
bun run dev:web        # Nuxt, :3000
```

`mprocs` bundles Postgres + backend + Nuxt into one multiplexed shell:

```bash
mprocs                          # docker postgres + mock backend + Nuxt
mprocs -c mprocs.live.yaml      # live local node at ws://127.0.0.1:9944
mprocs -c mprocs.prod.yaml      # public Allfeat mainnet archive RPC
```

Open <http://127.0.0.1:3000>.

The dev shell in `flake.nix` pins every tool the project expects (`cargo`,
`subxt` CLI, `bun`, `playwright`, `sqlx-cli`, `mprocs`). Use `nix develop`
or `direnv exec .` to enter it.

## Build-time configuration

Mock ↔ RPC is a Cargo feature; the two paths never coexist in one binary.

| Cargo feature | Effect                                                                                                |
|---------------|-------------------------------------------------------------------------------------------------------|
| `ssr`         | Base server build (axum, tokio, subxt, sqlx, tracing, moka, …). Required for every backend command.  |
| `mock`        | Swaps `RpcProvider` → `MockProvider` at compile time. Subxt and sqlx are never linked.                |
| `ts-bindings` | Activates the `ts-rs` derives for TS binding generation. Flipped only by `bun run gen:bindings`.       |

Production builds use `--features ssr` (no `mock`, no `ts-bindings`).

## Runtime configuration

Environment-only. `ServerConfig::from_env` is the sole reader at boot; the
request path never touches `std::env::var`.

| Variable                  | Default                         | Meaning                                                      |
|---------------------------|---------------------------------|--------------------------------------------------------------|
| `RPC_ENDPOINT_<NETWORK>`  | _unset ⇒ network disabled_      | RPC endpoint for a given `NetworkSpec::id` (upper-cased). Networks without an endpoint are hidden from the switcher and skipped by the indexer. Boot aborts in non-mock builds when no network has an endpoint set. |
| `LISTEN_ADDR`             | `127.0.0.1:8088`                | Socket the HTTP server binds to.                              |
| `WS_ALLOWED_ORIGINS`      | `*` (dev) — set explicitly in prod | Comma-separated origin allowlist for `/api/v1/live`.      |
| `API_ALLOWED_ORIGINS`     | `*` (dev) — set explicitly in prod | Comma-separated CORS allowlist for `/api/v1/*`.           |
| `DATABASE_URL`            | _unset_                         | Postgres connection string. Unset ⇒ indexer disabled, reads fall back to `RpcProvider`. One DB covers every indexed network (multi-tenant via a `network_id` / `network_sid` column). |
| `INDEXER_BACKFILL_CONCURRENCY` | `8`                        | Parallel backfill workers. Clamped ≥1.                        |
| `RUST_LOG`                | `info,allfeat_explorer=debug`   | `tracing-subscriber` filter.                                 |

Frontend runtime knobs live in `web/nuxt.config.ts` and are overridden via
`NUXT_*` env vars in prod — notably `NUXT_API_ORIGIN` (SSR target; Nitro
bypasses the Vite dev proxy) and `NUXT_PUBLIC_WS_BASE` (direct WS origin
because the Nuxt dev server cannot proxy WebSocket upgrades).

See [`docs/deployment.md`](docs/deployment.md) for the prod runbook (TLS,
reverse proxy, ArgoCD, SealedSecrets) and [`docs/ops.md`](docs/ops.md) for
operator playbooks (backfill, reconciliation, backups).

## Deployment modes

A single binary, three roles via `--mode`:

- `all` (default) — indexer workers + HTTP in one process. Tolerates a
  missing DB (falls back to `RpcProvider`).
- `indexer` — worker pool only, no HTTP listener. Requires `DATABASE_URL`.
- `server` — HTTP only, no workers. Requires `DATABASE_URL`.

`--reconcile` is a one-shot maintenance sweep: walk every row in
`account_balances`, refetch from the chain at the current head, UPDATE
drifted totals in place, exit. Run with the indexer stopped to avoid
self-healing races.

## Running against a live node

1. Start an Allfeat / Melodie node on `ws://127.0.0.1:9944`.
2. If the runtime changed, regenerate `artifacts/allfeat-metadata.scale`
   with the `subxt` CLI (see the memory note
   `project_metadata_generation.md` for the exact scope and command).
3. Start the stack:
   ```bash
   mprocs -c mprocs.live.yaml
   # …or by hand:
   docker compose up -d postgres
   RPC_ENDPOINT_MELODIE=ws://127.0.0.1:9944 \
     DATABASE_URL=postgres://explorer:explorer@127.0.0.1:54320/explorer \
     cargo run --features ssr --bin allfeat-explorer
   ```

The subxt client lives behind `RwLock<Option<AllfeatClient>>`: transient
WebSocket hiccups invalidate the slot and the next request reconnects. A
background supervisor subscribes to finalized headers and publishes the
head number into a watch channel, so hot paths read it as a single
in-process load rather than a round-trip per request.

## Testing

### Rust

```bash
# Unit tests — no external services.
cargo test --features ssr --lib

# Live RPC integration — needs a dev node on ws://127.0.0.1:9944.
# Override with ALLFEAT_RPC_URL. All such tests are #[ignore] by default.
cargo test --features ssr -- --ignored

# DB integration — needs the postgres-test service (port 54329).
docker compose up -d postgres-test
cargo test --features ssr -- --ignored
```

`tests/common/mod.rs::fresh_db` creates a throwaway `test_<pid>_<rand>`
database per test and schedules `DROP DATABASE` on teardown, so no state
leaks across runs.

### Playwright

```bash
bun run test:e2e
```

`end2end/playwright.config.ts` boots both servers automatically. The
backend runs with `--features ssr,mock` by default so the suite passes
without a live chain. Point the suite at a real node by exporting
`EXPLORER_FEATURES=ssr` plus `RPC_ENDPOINT_<NETWORK>=...`.

## Generated TypeScript bindings

Wire types live once in `src/domain.rs` and are exported to
`bindings/*.ts` via `ts-rs`. The Nuxt app consumes them through the
`@bindings` alias so the server and the browser share the same shapes.

```bash
bun run gen:bindings
```

Run this after any change to a type that carries
`#[cfg_attr(feature = "ts-bindings", derive(TS))]`, and commit the
regenerated files.

## Production build

```bash
# Rust binary.
bun run build:api
# Nuxt.
bun run build:web
```

Images are published to GHCR by `.github/workflows/images.yml` on every
push to `master` and every `v*` tag:

- `ghcr.io/allfeat/allfeat-explorer-backend` — one image, three roles
  (`--mode=all|indexer|server`) + the `--reconcile` one-shot.
- `ghcr.io/allfeat/allfeat-explorer-web`.

Kubernetes manifests live in the separate `infra-web3` repo; see
[`docs/deployment.md`](docs/deployment.md).

## Project layout

```
src/                      Rust backend (allfeat-explorer crate)
  data/                   ChainData trait + providers (rpc, indexed, mock)
  indexer/                live worker, backfill, projections, sink, reconcile
  server/                 Axum router, config, state, health, metrics
  live/                   WebSocket protocol + server fan-out
  network.rs              NetworkSpec + ChainCtx
  domain.rs               serializable wire types (source for bindings)
  mock/                   deterministic generators (feature = "mock")
artifacts/                pinned SCALE metadata for subxt codegen
bindings/                 generated *.ts (committed)
migrations/               sqlx migrations (baked into the binary)
web/                      Nuxt 4 frontend
  app/{components,composables,pages,stores,utils,...}
deploy/docker/            Dockerfile.backend + Dockerfile.web
end2end/                  Playwright suite
tests/                    Rust integration tests (all #[ignore])
docs/                     deployment + ops + plans
```

## Licensing

See [LICENSE](LICENSE).
