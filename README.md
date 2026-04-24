# Allfeat Explorer

Block explorer for the [Allfeat](https://github.com/Allfeat/allfeat) and
Melodie networks. Built with [Leptos 0.8](https://github.com/leptos-rs/leptos)
SSR + [Axum](https://github.com/tokio-rs/axum); on-chain data served through
[subxt](https://github.com/paritytech/subxt) against any Substrate-compatible
node, with a built-in mock backend for offline development.

## Quick start

```bash
# dev server backed by the synthetic mock generator (no node required)
cargo leptos watch --bin-features mock --lib-features mock

# dev server hitting a local Allfeat / Melodie node (default build, no extra features)
RPC_ENDPOINT_MELODIE=ws://127.0.0.1:9944 cargo leptos watch
```

Open <http://127.0.0.1:3000>.

The dev shell in `flake.nix` already pins every tool the project expects
(`cargo-leptos`, `wasm-bindgen-cli`, `dart-sass`, `pnpm`, `playwright`, the
`subxt` CLI). `nix develop` or `direnv exec .` picks it up.

## Build-time configuration

The mock ↔ RPC choice is a Cargo feature, not an env var:

| Cargo feature | Effect                                                                                                |
|---------------|-------------------------------------------------------------------------------------------------------|
| _(default)_   | Subxt-backed `RpcProvider`. Talks to the configured node. Mock module isn't compiled.                 |
| `mock`        | Deterministic in-memory generator. The RPC stack (subxt, moka, …) isn't compiled and never linked.   |

Pass it through cargo-leptos via `--bin-features mock --lib-features mock` (or
add a CI matrix that builds both). `cargo leptos serve` without the flag
yields the production build.

## Runtime configuration (RPC build only)

Configuration is environment-only — nothing is read outside of boot, so
`ServerConfig::from_env` is the single source of truth.

| Variable                  | Default                           | Meaning                                                         |
|---------------------------|-----------------------------------|-----------------------------------------------------------------|
| `RPC_ENDPOINT_<NETWORK>`  | _unset ⇒ network disabled_        | RPC endpoint for a given `NetworkSpec::id`. Networks without an endpoint are hidden from the UI switcher and skipped by the indexer. |
| `WS_ALLOWED_ORIGINS`      | `*` (dev) — set explicitly in prod | Comma-separated exact origin allowlist for `/ws` upgrades.      |
| `DATABASE_URL`            | _unset_                           | Postgres connection string for the indexer. One DB covers every indexed network (multi-tenant via `network_id` column). Unset ⇒ indexer disabled, reads fall back to `RpcProvider`. |
| `RUST_LOG`                | `info,allfeat_explorer=debug`     | `tracing-subscriber` filter. See "Logging" below.               |

For a production deployment (TLS, reverse-proxy config, systemd unit,
Docker + kube probes) see [docs/deployment.md](docs/deployment.md).

`<NETWORK>` is the upper-case network id (e.g. `RPC_ENDPOINT_MELODIE`,
`RPC_ENDPOINT_ALLFEAT`). Only the networks declared in
`src/network.rs::NETWORKS` are resolved. Networks are opt-in: if
`RPC_ENDPOINT_<ID>` is not set, that chain is completely disabled —
hidden from the switcher, not indexed, and never fetched. Boot aborts in
non-mock builds when **no** network has an endpoint configured; there
would be nothing to serve.

## Running against a live node

1. Start an Allfeat / Melodie dev node on `ws://127.0.0.1:9944`.
2. Regenerate `artifacts/allfeat-metadata.scale` if the runtime changed (the
   `subxt` CLI lives in the dev shell; see the memory note
   `project_metadata_generation.md` for the exact scope and command).
3. Launch the server (default build = RPC mode):
   ```bash
   RPC_ENDPOINT_MELODIE=ws://127.0.0.1:9944 \
     cargo leptos serve --release
   ```

The subxt client lives behind an `RwLock<Option<AllfeatClient>>`: transient
WebSocket hiccups invalidate the slot and the next request re-connects. On a
connect-time transport error `RpcClient::subxt` retries once before bubbling
up. A background task subscribes to finalized headers and publishes the head
number to a watch channel, so the hot paths read the finalized head as a
single in-process load instead of a round-trip per request.

## Logging

The server installs `tracing-subscriber` at startup and layers
`tower_http::trace::TraceLayer` on the axum router — every HTTP / WS
request produces a span with latency and status, and the RPC path emits
`debug` traces on cache misses and watcher restarts. Override the filter
via `RUST_LOG`:

```bash
RUST_LOG=debug,allfeat_explorer=trace cargo leptos serve
```

## Testing

### Rust — unit + ignored integration

```bash
# unit tests (no node needed)
cargo test --features ssr --lib

# live integration tests; requires ws://127.0.0.1:9944
cargo test --features ssr -- --ignored
# override the endpoint:
ALLFEAT_RPC_URL=ws://... cargo test --features ssr -- --ignored
```

Unit tests live in `#[cfg(test)] mod tests` blocks inside each module (mostly
`src/data/rpc/{mappers,cache,client}.rs`). Live tests live in
`tests/rpc_integration.rs` and are all `#[ignore]` — CI runs unit tests
unconditionally and opts into `--ignored` only when a node is available.

### End-to-end — Playwright

```bash
# from the repo root; the Playwright webServer block builds with
# `--bin-features mock --lib-features mock` so the suite passes without a node.
pnpm --dir end2end exec playwright test

# Or point at a live node — drop the mock features and forward RPC endpoints:
EXPLORER_FEATURES= RPC_ENDPOINT_MELODIE=ws://127.0.0.1:9944 \
  pnpm --dir end2end exec playwright test
```

Chromium is installed once via `pnpm --dir end2end exec playwright install
chromium`.

## Production build

```bash
cargo leptos build --release
```

Artifacts land in `target/release` (server binary) and `target/site` (static
assets). Copy both to the target host, set the `LEPTOS_*` env vars below, and
run the binary:

```sh
export LEPTOS_OUTPUT_NAME="allfeat-explorer"
export LEPTOS_SITE_ROOT="site"
export LEPTOS_SITE_PKG_DIR="pkg"
export LEPTOS_SITE_ADDR="127.0.0.1:3000"
export LEPTOS_RELOAD_PORT="3001"

export RPC_ENDPOINT_MELODIE=wss://rpc.melodie.allfeat.io
```

## Project layout

```
src/
  data/                trait ChainData + providers (mock when feature=mock, otherwise rpc/{client,cache,mappers,provider})
  server/              ServerConfig, AppState, #[server] fns per theme
  pages/               Leptos SSR pages (one per route)
  ui/                  header / footer / brand / network widgets
  format.rs            display helpers (planck → AFT, hash truncation, time deltas)
  network.rs           NetworkSpec definitions + ChainCtx (real-chain metadata)
  domain.rs            serializable types shipped over server fns
  mock/                synthetic data generators (gated behind feature = "mock")
artifacts/             pinned SCALE metadata for codegen
docs/                  migration plan & design notes
end2end/               Playwright suite
tests/                 Rust integration tests (all #[ignore])
```

## Licensing

See [LICENSE](LICENSE).
