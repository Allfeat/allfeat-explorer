import { expect, test } from "@playwright/test";

// Smoke — the dashboard's extrinsics/events panel must toggle between
// the two views without navigating. Prefix underscore to match the
// existing `_waveform-debug` convention for focused, single-feature
// specs that are run on demand rather than as part of the baseline
// sweep.

test.describe("dashboard extrinsics/events panel", () => {
  test("toggles between extrinsics and events views", async ({ page }) => {
    await page.goto("/");
    // Wait for hydration so click handlers are live before we interact.
    await page.waitForLoadState("networkidle");

    // The panel heading text changes with the toggle, so anchor on the
    // stable "Events" button that exists in both states instead.
    const panel = page.locator(".panel", { has: page.locator("button", { hasText: "Events" }) });
    await expect(panel).toBeVisible();

    const extBtn = panel.locator("button", { hasText: "Extrinsics" });
    const evtBtn = panel.locator("button", { hasText: "Events" });
    const heading = panel.locator("h3");

    // Initial state: Extrinsics tab active, call chips present.
    await expect(extBtn).toHaveClass(/active/);
    await expect(heading).toHaveText("Latest extrinsics");
    await expect(panel.locator(".chip.call").first()).toBeVisible();

    // Flip to events.
    await evtBtn.click();
    await expect(heading).toHaveText("Latest events");
    await expect(evtBtn).toHaveClass(/active/);
    await expect(panel.locator("tbody tr")).not.toHaveCount(0);
    // Events view labels rows with a Block # kicker and a phase sub-label
    // ("extrinsic N-M", "on_initialize", or "on_finalize").
    await expect(panel).toContainText(/extrinsic \d+-\d+|on_initialize|on_finalize/);

    // And back.
    await extBtn.click();
    await expect(heading).toHaveText("Latest extrinsics");
    await expect(extBtn).toHaveClass(/active/);
  });
});
