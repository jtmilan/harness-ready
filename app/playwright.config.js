import { defineConfig, devices } from "@playwright/test";

// Visual-regression harness for the agent-teams frontend. Renders app/src as a static page
// in Playwright's WEBKIT engine (closest safe parity to the real Tauri WKWebView — Chrome
// tools can't attach to a packaged Tauri app on macOS) with a mocked window.__TAURI__.
// Catches theme regressions (esp. the liquid-glass skin). NOT a real end-to-end harness.
const PORT = 5599;

export default defineConfig({
  testDir: "tests-visual",
  snapshotPathTemplate: "tests-visual/__screenshots__/{testFilePath}/{arg}-{projectName}{ext}",
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: [["list"]],
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 1,
  },
  expect: {
    // A retheme is a big intended change → baselines are regenerated with --update-snapshots.
    // Between runs, small anti-alias jitter is tolerated; structural drift fails.
    toHaveScreenshot: { maxDiffPixelRatio: 0.01, animations: "disabled" },
  },
  projects: [{ name: "webkit", use: { ...devices["Desktop Safari"] } }],
  webServer: {
    command: `python3 -m http.server ${PORT} --directory src`,
    url: `http://127.0.0.1:${PORT}/index.html`,
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
  },
});
