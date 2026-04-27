# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Stack overview

Block explorer for **Allfeat** (mainnet) and **Melodie** (testnet) Substrate chains. Post-rewrite the project is split into two processes:

- **Rust backend** (`src/`, crate `allfeat-explorer`) — Axum REST (`/api/v1/*`) + WebSocket live-stream on `:8088`. One binary, three roles (`--mode=all|indexer|server`). Talks to the chain via `subxt`, persists indexed state in Postgres, falls back to RPC when the DB is unset.
- **Nuxt 4 frontend** (`web/`) — Vue 3 SSR on `:3000`, proxies `/api/*` to the backend. Imports wire types from generated `bindings/*.ts` via the `@bindings` alias.

## Common commands

Root `package.json` orchestrates the whole stack through Bun:

| Command | Purpose |
|---|---|
| `bun run dev:api` | Rust backend with `--features ssr,mock` (no node, no DB). Sets `EXPLORER_DISABLE_RATE_LIMIT=1`. |
| `bun run dev:web` | `cd web && bun run dev` (Nuxt on `127.0.0.1:3000`). |
| `bun run build:api` | Release build: `cargo build --release --features ssr --bin allfeat-explorer`. |
| `bun run build:web` | `cd web && bun run build` (`nuxt build`). |
| `bun run typecheck` | `cd web && bun run typecheck` (vue-tsc). |
| `bun run gen:bindings` | Regenerate `bindings/*.ts` from ts-rs derives (`cargo test --features ts-bindings,ssr,mock --lib --quiet export_bindings_`). Run this whenever a `#[cfg_attr(feature = "ts-bindings", derive(TS))]` type changes. |
| `bun run test:e2e` | `cd end2end && bun x playwright test`. |

`mprocs` panes glue DB + API + web together:

- `mprocs` — default (`mprocs.yaml`): docker postgres + mock API + Nuxt.
- `mprocs -c mprocs.live.yaml` — live local node at `ws://127.0.0.1:9944` + DB + Nuxt.
- `mprocs -c mprocs.prod.yaml` — public Allfeat mainnet archive RPC (only `allfeat` enabled; Melodie hidden).

Feature-flag conventions matter:

- **`ssr`** — enables the server (axum, subxt, sqlx, tracing, moka, …). Default for every backend command.
- **`mock`** — swaps `RpcProvider` → `MockProvider` at compile time; subxt and sqlx are never linked. Used for dev without a node, Playwright, and design iteration.
- **`ts-bindings`** — activates the ts-rs `#[ts(export)]` attributes; only flipped by `gen:bindings`. Never ship this in prod builds.

`mock` is compile-time, not runtime — the two paths can't coexist in one binary.

## Testing

### Rust

```bash
# Unit tests — no external services.
cargo test --features ssr --lib

# Live RPC integration (tests/rpc_integration.rs, tests/banner.rs, …).
# Needs a dev node on ws://127.0.0.1:9944. Override via ALLFEAT_RPC_URL.
cargo test --features ssr -- --ignored

# DB integration tests use the fresh_db() helper in tests/common/mod.rs.
# Requires the postgres-test service from docker-compose.yml (port 54329).
# Override admin URL via TEST_DATABASE_URL.
docker compose up -d postgres-test
cargo test --features ssr -- --ignored
```

All integration tests are `#[ignore]` by default; CI runs `--ignored` only when the services are up. `tests/common/mod.rs::fresh_db` creates a jetable `test_<pid>_<rand>` DB per test and schedules `DROP DATABASE` on teardown.

### Playwright

`end2end/playwright.config.ts` boots both servers automatically. By default the backend runs with `--features ssr,mock`; set `EXPLORER_FEATURES=ssr` plus `RPC_ENDPOINT_<NETWORK>` to run the suite against a real node. Uses `bun x playwright test` from the project.

## Architecture — the big picture

### Provider stack (`src/data/`)

Everything routes through the async trait `ChainData` (`src/data/provider.rs`). Three implementations, picked at boot by `AppState::from_config` in `src/server/state.rs`:

1. `MockProvider` (feature = `mock` only).
2. `RpcProvider` (subxt-backed; one `RpcClient` per configured network). `RpcClient` holds `RwLock<Option<AllfeatClient>>` — transient WS errors invalidate the slot and the next call reconnects. A supervisor task subscribes to finalized headers and publishes the head number onto a watch channel, so request-hot paths read the head as a single in-process load.
3. `IndexedProvider` — wraps `RpcProvider` and takes over each method as it gets migrated to Postgres reads; untouched methods still hit RPC. Present only when `DATABASE_URL` is set.

All write SQL is funnelled through `src/indexer/sink.rs`; projections (`src/indexer/projections/*.rs`) stay pure. `ON CONFLICT DO NOTHING` everywhere — the live worker and the backfill runners race safely on overlapping ranges.

### Multi-tenant indexing

One deployment indexes **every** configured network into **one** Postgres database. Every table carries `network_id` (TEXT) interned to `network_sid` (SMALLINT) via `src/indexer/lookups.rs::NetworkLookup`. The lookup is seeded once post-migration in `AppState::seed_lookups`; every query site resolves `&str → SMALLINT` through it. `authors` gets the same treatment.

Networks are opt-in: a network declared in `src/network.rs::NETWORKS` is only enabled if `RPC_ENDPOINT_<ID>` is set at boot. Unset networks are invisible everywhere — switcher, indexer, API.

### Deployment modes (`--mode`)

`src/main.rs` parses `--mode=all|indexer|server` (default `all`):

- `all` — writes + serves HTTP. Tolerates missing DB (falls back to RPC).
- `indexer` — writes only, no HTTP listener. Requires `DATABASE_URL`.
- `server` — reads only, no indexer workers. Requires `DATABASE_URL`.

`--reconcile` short-circuits boot: walks every row in `account_balances`, refetches from the chain at the current head, UPDATEs drifted totals, and exits. Run with the indexer stopped to avoid self-healing races.

### Live stream (`src/live/`)

One multiplexed WebSocket per browser tab at `/api/v1/live`. Topics: blocks / transfers / ats feed. The server owns a per-network producer task fed by the finalized-head watcher; subscriptions fan out via `tokio::sync::broadcast`. Browser-side client is `web/app/composables/useLiveSocket.ts` — a module-level singleton keyed by active network. **Nuxt's dev server does not proxy WebSocket upgrades** (Nitro/h3 intercepts them), so the browser targets `runtimeConfig.public.wsBase` directly; prod reverse-proxy handles both HTTP and WS uniformly.

### Config (`src/server/config.rs`)

`ServerConfig::from_env` is the sole env-var reader — nothing else calls `std::env::var` at request time. The most load-bearing variables:

| Variable | Purpose |
|---|---|
| `RPC_ENDPOINT_<NETWORK>` | Per-network endpoint (opt-in). `<NETWORK>` is the upper-cased `NetworkSpec::id`. |
| `DATABASE_URL` | Postgres; unset ⇒ no indexer, RPC fallback for everything. |
| `LISTEN_ADDR` | Default `127.0.0.1:8088`. |
| `WS_ALLOWED_ORIGINS` / `API_ALLOWED_ORIGINS` | Origin allowlists. `*` = any (dev default; prod must set). |
| `INDEXER_BACKFILL_CONCURRENCY` | Default 8; clamped ≥1. |
| `NUXT_API_ORIGIN` | Web-side: SSR `$fetch` target. Nitro's internal dispatcher bypasses the Vite dev proxy, so SSR needs an absolute backend URL. |
| `NUXT_PUBLIC_WS_BASE` | Web-side: WebSocket origin (see live-stream note above). |
| `GIT_SHA` | Pre-populated in CI/Docker builds; `build.rs` falls back to `git rev-parse --short=7 HEAD`. Exposed via `/api/v1/meta` and the footer. |

Boot aborts in non-mock builds if **no** `RPC_ENDPOINT_*` is set — nothing to serve.

### Subxt metadata

Runtime codegen lives in `src/data/rpc/runtime.rs` as two sibling modules — `pub mod allfeat {}` against `artifacts/allfeat-metadata.scale` and `pub mod melodie {}` against `artifacts/melodie-metadata.scale`. The macro is compile-time, so dispatch happens at the call site: every mapper / projection / decoder takes a `runtime_kind: RuntimeKind` (carried by `RpcClient`, sourced from `NetworkSpec.runtime_kind`) and matches on it before reaching into the codegen. Both modules share `SubstrateConfig` (AccountId32 / MultiAddress / BlakeTwo256 / u32), so on-the-wire shapes overlap and only `runtime_types::*` differ between runtimes.

Static metadata + decoders live in `src/data/metadata.rs`: `ALLFEAT_RUNTIME` / `MELODIE_RUNTIME` (`RuntimeMetadata { bytes, version, decoded: LazyLock<Metadata> }`) plus a `runtime_for(network_id)` dispatcher. All public decoders (`decode_event_fields`, `decode_call_args`, `decode_call_fields`, `callable_pallet_names`, `metadata_version_for`, `metadata_bytes_for`) take a `network_id` and route through it.

Regeneration scope and per-chain commands are tracked in the user's memory file `project_metadata_generation.md`.

Default subxt config for this project is `SubstrateConfig`, not `PolkadotConfig` (see `feedback_subxt_config.md`). Always consult `paritytech/subxt`'s `subxt/examples/` when touching subxt code (`reference_subxt_examples.md`).

### Frontend layout (`web/app/`)

- `srcDir` is `app/`. Components auto-import flat (no prefix): `components/ui/Hash.vue` → `<Hash/>`.
- `components/` is feature-grouped (`dashboard`, `detail`, `hero`, `layout`, `live`, `runtime`, `tables`, `token`, `ui`).
- `composables/` — one per data surface (`useLiveBlocks`, `useLiveWaveform`, `usePaginatedList`, etc.); the raw socket lives in `useLiveSocket.ts`.
- `stores/` — Pinia (`live.ts` for connection/buffer state, `networks.ts` for the active-network selector).
- `utils/` — SS58 encoding, identicon, known-account labels, formatting. **Static address→label mappings belong here + a composable, not a backend endpoint** (see `feedback_frontend_only_labels.md`).
- `bindings/*.ts` is generated from Rust; import shared wire types from `@bindings` rather than redeclaring them.
- In `<style scoped>` blocks Vue uses PostCSS, which crashes on `//` comments — always use `/* */` (see `feedback_vue_style_blocks_comment_syntax.md`).

## Tooling

- Rust toolchain is pinned in `rust-toolchain.toml` (stable, MSRV 1.82, with `wasm32-unknown-unknown` target retained for historical reasons).
- `rustfmt.toml` sets `max_width = 100`; `.clippy.toml` is minimal.
- `flake.nix` provides the dev shell (cargo, subxt CLI, bun, playwright, sqlx-cli, mprocs). Use `nix develop` or `direnv exec .`.
- Migrations live in `migrations/` and are baked into the binary via `sqlx::migrate!`; `MIGRATOR.run(&pool)` fires at boot when `DATABASE_URL` is set, so operators don't run `sqlx migrate` out-of-band.
- Two container images published to GHCR (`.github/workflows/images.yml`): `allfeat-explorer-backend` (one image, two roles via `--mode`) and `allfeat-explorer-web`. Kubernetes manifests live in the separate `infra-web3` repo; see `docs/deployment.md`.
- Versioning is automated via `release-please` (`.github/workflows/release-please.yml`, `release-please-config.json`). Each package versions independently with namespaced tags (`backend-v*`, `web-v*`); merging a release PR triggers `images.yml`, which builds the matching image and bumps the corresponding `newTag` in `Allfeat/infra-web3` for ArgoCD to pick up.

## Conventions

- **Git commits must never include `Co-Authored-By: Claude` or any Claude attribution.** Commit under the user's configured git identity only — this overrides Claude Code's default commit template (see `feedback_no_claude_attribution.md`).
- Use [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `perf:`, `refactor:`, `docs:`, `style:`, `test:`, `ci:`, `chore:`); `feat`/`fix`/`perf` drive automated version bumps via release-please. Scope by package when relevant (`feat(api):`, `fix(web):`).
