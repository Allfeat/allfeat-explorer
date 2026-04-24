import { expect, test } from "@playwright/test";

// Live-layer smoke — asserts the WebSocket plugin wakes up post-hydrate
// and flips the connection indicator to `Live`, then confirms a newer
// block arrives within the mock's block-time window.
//
// The footer chip's BLOCK readout is driven by the live Pinia store,
// which is both SSR-seeded and live-pushed. We don't assert on the
// finalized half of the chip: the mock emits unfinalized tips and only
// retroactively marks them finalized once they've fallen 2+ behind
// head, so a short test window can't guarantee a finalized block is
// still in the 25-block buffer after a few client-side pushes.

test.describe("live layer", () => {
  test("connection pill flips to Live after hydrate", async ({ page }) => {
    await page.goto("/");
    const pill = page.locator(".connection-pill");
    await expect(pill).toBeVisible();
    // SSR renders `connecting` (plugin is client-only). After hydration
    // the WS handshake completes and the pill swaps to `--connected`.
    await expect(pill).toHaveClass(/connection-pill--connected/, { timeout: 20_000 });
    await expect(pill).toContainText("Live");
  });

  test("footer head chip carries a real block number", async ({ page }) => {
    await page.goto("/");
    const chip = page.locator(".footer-head-chip");
    await expect(chip).toBeVisible();
    // Narrow non-breaking space separator; match BLOCK + digits.
    await expect(chip).toContainText(/BLOCK\s+[\d\u202F]+/);
  });

  test("new block arrives over the live socket", async ({ page }) => {
    await page.goto("/");
    const chip = page.locator(".footer-head-chip");
    // Wait for the WS handshake before reading the baseline — an
    // SSR-seeded number could otherwise race with the first live push
    // and either match "no change" or already-advanced by the time the
    // poll starts.
    await expect(page.locator(".connection-pill")).toHaveClass(
      /connection-pill--connected/,
      { timeout: 20_000 },
    );
    const initial = await chip.textContent();
    // Allfeat mock block time is 6s; give ourselves two windows.
    await expect.poll(
      async () => chip.textContent(),
      { timeout: 15_000, intervals: [500, 1000, 2000] },
    ).not.toBe(initial);
  });
});
