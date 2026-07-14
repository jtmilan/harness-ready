import { test, expect } from "@playwright/test";
import { installMockTauri } from "./support/mock-tauri.js";

// Theme visual-regression + the liquid-glass RENDER CHECK. Renders the empty app shell
// (rail + topbar + launcher; no panes) per skin, masks the terminal so xterm churn can't
// cause false diffs, and asserts no page errors. Baselines are (re)generated with
// `npx playwright test --update-snapshots`.
// All 6 skins — keep in sync with app/src/skin-core.js SKINS (this spec runs pre-bundle
// against the raw page, so it mirrors the list by hand like the index.html pre-paint copy).
const SKINS = ["nothing", "liquid-glass", "aurora", "atelier", "phosphor", "precision"];

for (const skin of SKINS) {
  test(`shell renders — skin=${skin}`, async ({ page }) => {
    const pageErrors = [];
    const consoleErrors = [];
    page.on("pageerror", (e) => pageErrors.push(String(e)));
    page.on("console", (m) => {
      if (m.type() === "error") consoleErrors.push(m.text());
    });

    await page.addInitScript(installMockTauri);
    await page.goto(`/index.html?skin=${skin}`);

    // Shell is up.
    await expect(page.locator("#rail")).toBeVisible();
    await expect(page.locator("#topbar")).toBeVisible();

    // The requested skin actually applied (nothing = the bare :root, no attribute).
    if (skin === "nothing") {
      await expect(page.locator("html")).not.toHaveAttribute("data-skin", /.+/);
    } else {
      await expect(page.locator("html")).toHaveAttribute("data-skin", skin);
    }

    // Deterministic settle (replaces the flat 400ms sleep): every declared font loaded,
    // then two rAFs so the post-font relayout has painted, plus a tiny cushion for
    // xterm's async first render (masked anyway, but keeps toHaveScreenshot retries calm).
    await page.evaluate(() => document.fonts.ready);
    await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
    await page.waitForTimeout(100);

    await expect(page).toHaveScreenshot(`shell-${skin}.png`, {
      mask: [page.locator("#terminal")], // terminal stays out of the baseline
      fullPage: false,
    });

    // RENDER CHECK: a real JS exception during boot/skin-apply is always a failure.
    expect(pageErrors, `page errors under skin=${skin}: ${pageErrors.join(" | ")}`).toEqual([]);
    // liquid-glass is the new skin — hold it to a stricter no-console-error bar too.
    if (skin === "liquid-glass") {
      expect(
        consoleErrors,
        `console errors under liquid-glass: ${consoleErrors.join(" | ")}`
      ).toEqual([]);
    }
  });
}
