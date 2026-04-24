import { test } from "@playwright/test";

test("verify sweep fade", async ({ browser }) => {
  const context = await browser.newContext({ javaScriptEnabled: false, colorScheme: "dark" });
  const page = await context.newPage();
  await page.goto("/", { waitUntil: "load" });
  const data = await page.evaluate(() => {
    const sweep = document.querySelector('.wh-sweep');
    return {
      has_class: !!sweep,
      style: sweep?.getAttribute('style'),
      x: sweep?.getAttribute('x'),
    };
  });
  console.log('SWEEP_SSR:', JSON.stringify(data));
  await context.close();
});
