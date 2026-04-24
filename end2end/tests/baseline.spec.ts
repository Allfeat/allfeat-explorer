import { expect, test } from "@playwright/test";

// Baseline smoke — visits every route and asserts the SSR payload renders
// the expected markers (hero / breadcrumb / page-title / table rows).
// Relies on the default `waitUntil: 'load'` of `page.goto`: SSR content
// is present in the initial HTML, so we don't need to wait for client
// hydration here. Interactive coverage lives in `hydration.spec.ts`,
// live coverage in `live.spec.ts`.
//
// Real ids/addresses are pulled from the backend REST API (same origin
// as the frontend via Nuxt's dev proxy) so the assertions stay meaningful
// even after the mock provider's deterministic seed rotates.

const API = "/api/v1/networks/allfeat";

test.describe("explorer pages (mock)", () => {
  test("dashboard SSR hero", async ({ page }) => {
    await page.goto("/");
    await expect(page).toHaveTitle(/Allfeat Explorer/);
    await expect(page.locator(".hero-card")).toBeVisible();
    await expect(page.locator(".hero-card")).toContainText("Head block");
    await expect(page.locator(".hero-card")).toContainText("Next in");
    await expect(page.locator(".panel").filter({ hasText: "Latest blocks" })).toBeVisible();
    await expect(page.locator(".panel").filter({ hasText: "Latest extrinsics" })).toBeVisible();
  });

  test("blocks list", async ({ page }) => {
    await page.goto("/blocks");
    await expect(page.locator(".page-title h1")).toHaveText("Blocks");
    await expect(page.locator("table.table tbody tr").first()).toBeVisible();
  });

  test("block detail", async ({ page }) => {
    await page.goto("/blocks/42");
    await expect(page.locator(".breadcrumb")).toContainText("Blocks");
    await expect(page.locator(".page-title h1")).toContainText("#42");
  });

  test("extrinsics list", async ({ page }) => {
    await page.goto("/extrinsics");
    await expect(page.locator(".page-title h1")).toHaveText("Extrinsics");
    await expect(page.locator("table.table tbody tr").first()).toBeVisible();
  });

  test("extrinsic detail", async ({ page, request }) => {
    // `/extrinsics` now returns `{ items, page_info }` (Page<Extrinsic>).
    const resp = await request.get(`${API}/extrinsics?count=1`);
    expect(resp.ok()).toBe(true);
    const body = (await resp.json()) as { items: Array<{ id: string }> };
    const [first] = body.items;
    expect(first?.id).toMatch(/^\d+-\d+$/);
    await page.goto(`/extrinsics/${first.id}`);
    await expect(page.locator(".page-title h1")).toContainText(first.id);
  });

  test("accounts list", async ({ page }) => {
    await page.goto("/accounts");
    await expect(page.locator(".page-title h1")).toHaveText("Accounts");
    await expect(page.locator("table.table tbody tr").first()).toBeVisible();
  });

  test("account detail", async ({ page, request }) => {
    // `/accounts` now returns `{ items, page_info }` (Page<Account>).
    const resp = await request.get(`${API}/accounts?count=1`);
    expect(resp.ok()).toBe(true);
    const body = (await resp.json()) as { items: Array<{ address: string }> };
    const [first] = body.items;
    expect(first?.address).toBeTruthy();
    await page.goto(`/accounts/${first.address}`);
    await expect(page.locator(".page-title.account-title")).toBeVisible();
    await expect(page.locator(".panel").filter({ hasText: "Balance" })).toBeVisible();
  });

  test("ats list", async ({ page }) => {
    await page.goto("/ats");
    await expect(page.locator(".ats-h1")).toContainText("heartbeat");
    await expect(page.locator("table.table tbody tr").first()).toBeVisible();
  });

  test("ats detail", async ({ page }) => {
    // The backend route is `/ats/{reverse_index}` — index 0 is always
    // the newest registration, which the mock guarantees exists once
    // `ats_total > 0`. Asserting on the rendered ats_id rather than the
    // URL param lets us stay agnostic to that translation.
    await page.goto(`/ats/0`);
    await expect(page.locator(".breadcrumb")).toContainText("ATS");
    await expect(page.locator(".page-title h1")).toContainText(/^#\d+$/);
  });

  test("melodie network switch preserved via query", async ({ page }) => {
    await page.goto("/?network=melodie");
    await expect(page.locator(".network-switch .network-name")).toHaveText("Melodie");
    await expect(page.locator(".network-switch .network-dot.testnet")).toBeVisible();
  });
});
