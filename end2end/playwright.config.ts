import { defineConfig, devices } from "@playwright/test";

/**
 * End-to-end tests for the Allfeat Explorer (post-rewrite).
 *
 * ## Servers under test
 *
 * The v3 stack is a Rust backend on `:8088` (axum REST + WebSocket) paired
 * with a Nuxt 4 frontend on `:3000` that SSRs the UI and proxies `/api/v1`
 * calls back to the backend. Playwright boots both via the `webServer`
 * array below: the backend first, the Nuxt dev server second (Playwright
 * waits for each `url` to respond before starting the suite).
 *
 * ## Data source
 *
 * Mock vs RPC is a build-time Cargo feature. The default backend command
 * ships with `--features ssr,mock` so the suite passes without a live
 * chain. To point the suite at a live node instead, export
 * `EXPLORER_FEATURES=ssr` (or any other feature set without `mock`) and
 * forward `RPC_ENDPOINT_<NETWORK>` for each non-default endpoint.
 */

const BASE_URL = "http://127.0.0.1:3000";
const API_URL = "http://127.0.0.1:8088";

const features = process.env.EXPLORER_FEATURES ?? "ssr,mock";
const backendCmd = `cargo run --quiet --features ${features} --bin allfeat-explorer`;

// Playwright fans out one browser per CPU core and can easily exceed
// the prod-grade REST governor (100 req/s) during the baseline sweep.
// The backend honours `EXPLORER_DISABLE_RATE_LIMIT=1` as an escape hatch
// for test + dev runs; real deploys leave it unset.
const backendEnv: Record<string, string> = {
  EXPLORER_DISABLE_RATE_LIMIT: "1",
};
for (const [k, v] of Object.entries(process.env)) {
  if (k.startsWith("RPC_ENDPOINT_") && v) {
    backendEnv[k] = v;
  }
}

export default defineConfig({
  testDir: "./tests",
  timeout: 60_000,
  expect: { timeout: 10_000 },
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: "html",
  use: {
    actionTimeout: 0,
    baseURL: BASE_URL,
    trace: "on-first-retry",
  },
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
  ],
  webServer: [
    {
      command: backendCmd,
      cwd: "..",
      url: `${API_URL}/api/v1/healthz`,
      env: backendEnv,
      reuseExistingServer: !process.env.CI,
      timeout: 300_000,
      stdout: "pipe",
      stderr: "pipe",
    },
    {
      command: "bun run dev",
      cwd: "../web",
      url: BASE_URL,
      reuseExistingServer: !process.env.CI,
      timeout: 180_000,
      stdout: "pipe",
      stderr: "pipe",
    },
  ],
});
