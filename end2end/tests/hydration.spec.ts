import { expect, test, type Page } from "@playwright/test";

// Hydration smoke — exercises the URL-driven interactive bits that only
// work after Nuxt has hydrated on the client: tabs, filter segments,
// pagination, network switch. Each assertion pins the `?query=` change
// the maquette was specced around, which doubles as a regression guard
// against router misconfiguration.
//
// In dev mode Vite compiles modules on-demand, so first-load hydration
// can take several seconds. `waitForHydration` uses the connection-pill
// class swap (the WS plugin is the last thing that fires after the page
// is interactive) as the signal that Vue is wired up — clicks issued
// before that are lost because the event listener isn't bound yet.

async function waitForHydration(page: Page): Promise<void> {
  await expect(page.locator(".connection-pill")).toHaveClass(
    /connection-pill--connected/,
    { timeout: 20_000 },
  );
}

test.describe("hydration interactions", () => {
  test("block detail tabs toggle ?tab=", async ({ page }) => {
    await page.goto("/blocks/42");
    await waitForHydration(page);
    const eventsTab = page.locator(".tabs .tab", { hasText: "Events" });
    await eventsTab.click();
    await expect(page).toHaveURL(/\?tab=events/);
    await expect(eventsTab).toHaveClass(/active/);
  });

  test("blocks filter segment updates ?filter=", async ({ page }) => {
    await page.goto("/blocks");
    await waitForHydration(page);
    const finalized = page.locator(".seg button", { hasText: "Finalized" });
    await finalized.click();
    await expect(page).toHaveURL(/\?filter=finalized/);
    await expect(finalized).toHaveClass(/active/);
  });

  test("blocks pagination navigates to ?page=2", async ({ page }) => {
    await page.goto("/blocks");
    await waitForHydration(page);
    const pageTwo = page.locator(".pagination button", { hasText: /^2$/ }).first();
    await expect(pageTwo).toBeVisible();
    await pageTwo.click();
    await expect(page).toHaveURL(/\?page=2/);
    await expect(page.locator(".pagination .current")).toHaveText("2");
  });

  test("network switch opens menu and selects melodie", async ({ page }) => {
    await page.goto("/");
    await waitForHydration(page);
    await expect(page.locator(".network-switch .network-name")).toHaveText("Allfeat");
    await page.locator(".network-switch").click();
    const menuItem = page.locator(".network-menu .network-menu__item", { hasText: "Melodie" });
    await expect(menuItem).toBeVisible();
    await menuItem.click();
    await expect(page).toHaveURL(/\?.*network=melodie/);
    await expect(page.locator(".network-switch .network-name")).toHaveText("Melodie");
  });
});
