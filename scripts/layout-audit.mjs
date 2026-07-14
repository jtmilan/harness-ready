#!/usr/bin/env node
// layout-audit.mjs — headless multi-viewport layout checker for the Agent Teams frontend.
//
//   node scripts/layout-audit.mjs [--screens out/dir]
//
// Serves app/src statically, loads index.html in headless chromium (playwright-core
// from the npx cache — no new deps) with a mock window.__TAURI__, and at each
// viewport checks: no horizontal overflow; each modal overlay's .modal-card fits the
// viewport, can scroll its own overflow, and is actually on top (elementFromPoint —
// the operator's "rail paints over the Bridge modal" bug); bridge-dock causes no
// h-overflow. PASS/FAIL lines are machine-parseable: RESULT\t<WxH>\t<check>\t<status>\t<detail>
// Exits non-zero if any check FAILs. Failing states are screenshotted into --screens.

import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, "..", "app", "src");
const VIEWPORTS = [
  { width: 900, height: 700 },
  { width: 1280, height: 800 },
  { width: 1600, height: 1000 },
];
const OVERLAYS = ["bridge", "flywheel", "delegate", "history", "modal", "speak", "wizard", "delegations"];
const MARGIN = 8; // px tolerance for "card fits viewport"

const argv = process.argv.slice(2);
const si = argv.indexOf("--screens");
const screensDir = path.resolve(si >= 0 && argv[si + 1] ? argv[si + 1] : ".paul/analysis/ui-small-window/screens");
fs.mkdirSync(screensDir, { recursive: true });

// ---- locate playwright-core (npx cache or app/node_modules) + a cached chromium ----
function findPlaywright() {
  const candidates = [];
  const npx = path.join(os.homedir(), ".npm", "_npx");
  if (fs.existsSync(npx)) {
    for (const d of fs.readdirSync(npx)) {
      const p = path.join(npx, d, "node_modules", "playwright-core");
      if (fs.existsSync(path.join(p, "package.json"))) candidates.push(p);
    }
  }
  const local = path.resolve(__dirname, "..", "app", "node_modules", "playwright-core");
  if (fs.existsSync(path.join(local, "package.json"))) candidates.push(local);

  const cache = path.join(os.homedir(), "Library", "Caches", "ms-playwright");
  const exeFor = (rev) => {
    const tries = [
      path.join(cache, `chromium_headless_shell-${rev}`, "chrome-headless-shell-mac-arm64", "chrome-headless-shell"),
      path.join(cache, `chromium_headless_shell-${rev}`, "chrome-headless-shell-mac-x64", "chrome-headless-shell"),
      path.join(cache, `chromium-${rev}`, "chrome-mac", "Chromium.app", "Contents", "MacOS", "Chromium"),
    ];
    return tries.find((t) => fs.existsSync(t));
  };
  // Prefer a copy whose pinned chromium revision is actually in the cache.
  for (const c of candidates) {
    try {
      const bj = JSON.parse(fs.readFileSync(path.join(c, "browsers.json"), "utf8"));
      const rev = bj.browsers.find((b) => b.name === "chromium")?.revision;
      const exe = rev && exeFor(rev);
      if (exe) return { pkg: c, exe };
    } catch { /* keep looking */ }
  }
  // Fallback: any copy + any cached chromium executable.
  if (candidates.length && fs.existsSync(cache)) {
    for (const d of fs.readdirSync(cache)) {
      const rev = d.match(/^chromium(?:_headless_shell)?-(\d+)$/)?.[1];
      const exe = rev && exeFor(rev);
      if (exe) return { pkg: candidates[0], exe };
    }
  }
  return null;
}

// ---- tiny static server rooted at app/src ----
const MIME = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png", ".json": "application/json", ".woff2": "font/woff2", ".wasm": "application/wasm", ".map": "application/json" };
function startServer() {
  return new Promise((resolve) => {
    const srv = http.createServer((req, res) => {
      const url = decodeURIComponent(new URL(req.url, "http://x").pathname);
      let file = path.normalize(path.join(ROOT, url === "/" ? "index.html" : url));
      if (!file.startsWith(ROOT)) { res.writeHead(403); return res.end(); }
      fs.readFile(file, (err, data) => {
        if (err) { res.writeHead(404); return res.end("not found"); }
        res.writeHead(200, { "content-type": MIME[path.extname(file)] || "application/octet-stream" });
        res.end(data);
      });
    });
    srv.listen(0, "127.0.0.1", () => resolve(srv));
  });
}

const TAURI_MOCK = `
  window.__TAURI__ = {
    core: { invoke: async (cmd) => (/^list_|_history$|_ids$|^diff_changed_files$/.test(cmd) ? [] : {}) },
    event: { listen: async () => (() => {}) },
  };
`;

const results = []; // { vp, check, status, detail }
function record(vp, check, status, detail = "") {
  results.push({ vp, check, status, detail });
  console.log(`RESULT\t${vp}\t${check}\t${status}\t${detail}`);
}

async function shot(page, vp, check) {
  const name = `${vp}-${check.replace(/[^a-z0-9.-]+/gi, "_")}.png`;
  try { await page.screenshot({ path: path.join(screensDir, name) }); } catch { /* best-effort */ }
}

async function auditViewport(browser, baseUrl, viewport) {
  const vp = `${viewport.width}x${viewport.height}`;
  const ctx = await browser.newContext({ viewport });
  const page = await ctx.newPage();
  await page.addInitScript(TAURI_MOCK);
  await page.goto(baseUrl, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("#app", { state: "attached", timeout: 10000 });
  await page.waitForTimeout(500);

  // 1. baseline horizontal overflow
  const over = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  if (over > 1) { record(vp, "no-h-overflow", "FAIL", `scrollWidth exceeds viewport by ${over}px`); await shot(page, vp, "no-h-overflow"); }
  else record(vp, "no-h-overflow", "PASS");

  // 2. each modal overlay, one at a time
  for (const id of OVERLAYS) {
    // un-hide first, then let the card-in/overlay-in entrance animations finish
    // (240ms max — measuring at frame 0 reads opacity:0 + a translated rect)
    const pre = await page.evaluate((id) => {
      const ov = document.getElementById(id);
      if (!ov) return { skip: `no #${id} in DOM` };
      ov.classList.remove("hidden");
      if (!ov.querySelector(".modal-card")) { ov.classList.add("hidden"); return { skip: `#${id} has no .modal-card` }; }
      return {};
    }, id);
    if (pre.skip) { record(vp, `modal-${id}`, "SKIP", pre.skip); continue; }
    await page.waitForTimeout(400);
    const r = await page.evaluate(({ id, MARGIN }) => {
      const ov = document.getElementById(id);
      const card = ov.querySelector(".modal-card");
      const rect = card.getBoundingClientRect();
      const vw = window.innerWidth, vh = window.innerHeight;
      const fit = rect.left >= -MARGIN && rect.top >= -MARGIN && rect.right <= vw + MARGIN && rect.bottom <= vh + MARGIN;
      const cs = getComputedStyle(card);
      const needsScroll = card.scrollHeight > card.clientHeight + 1;
      const scrollOk = !needsScroll || cs.overflowY === "auto" || cs.overflowY === "scroll";
      const desc = (el) => (el ? el.tagName.toLowerCase() + (el.id ? "#" + el.id : "") + (el.classList[0] ? "." + el.classList[0] : "") : "null");
      const clampX = (x) => Math.min(Math.max(x, 0), vw - 1);
      const clampY = (y) => Math.min(Math.max(y, 0), vh - 1);
      const cx = clampX(rect.left + rect.width / 2), cy = clampY(rect.top + rect.height / 2);
      const atCenter = document.elementFromPoint(cx, cy);
      const atLeft = document.elementFromPoint(clampX(rect.left + 4), cy);
      // top-left corner too: sticky rail/topbar headers (z-index:1) paint over a
      // z:auto overlay exactly there — the operator's "rail over the modal" bug.
      const atTopLeft = document.elementFromPoint(clampX(rect.left + 4), clampY(rect.top + 4));
      const stackOk = ov.contains(atCenter) && ov.contains(atLeft) && ov.contains(atTopLeft);
      const out = {
        fit, fitDetail: `card ${Math.round(rect.width)}x${Math.round(rect.height)} @(${Math.round(rect.left)},${Math.round(rect.top)})..(${Math.round(rect.right)},${Math.round(rect.bottom)}) in ${vw}x${vh}`,
        scrollOk, scrollDetail: needsScroll ? `scrollHeight ${card.scrollHeight} > clientHeight ${card.clientHeight}, overflowY=${cs.overflowY}` : "content fits, no scroll needed",
        stackOk, stackDetail: `center→${desc(atCenter)} leftCenter→${desc(atLeft)} topLeft→${desc(atTopLeft)}`,
      };
      if (out.fit && out.scrollOk && out.stackOk) ov.classList.add("hidden"); // keep failing state visible for the screenshot
      return out;
    }, { id, MARGIN });

    record(vp, `modal-${id}.fit`, r.fit ? "PASS" : "FAIL", r.fitDetail);
    record(vp, `modal-${id}.scroll`, r.scrollOk ? "PASS" : "FAIL", r.scrollDetail);
    record(vp, `modal-${id}.stacking`, r.stackOk ? "PASS" : "FAIL", r.stackDetail);
    if (!(r.fit && r.scrollOk && r.stackOk)) {
      await shot(page, vp, `modal-${id}`);
      await page.evaluate((id) => document.getElementById(id).classList.add("hidden"), id);
    }
  }

  // 3. bridge-dock must not introduce horizontal overflow
  const dock = await page.evaluate(() => {
    const d = document.getElementById("bridge-dock");
    if (!d) return { skip: "no #bridge-dock in DOM" };
    d.classList.remove("hidden");
    const over = document.documentElement.scrollWidth - window.innerWidth;
    if (over <= 1) d.classList.add("hidden");
    return { over };
  });
  if (dock.skip) record(vp, "bridge-dock", "SKIP", dock.skip);
  else if (dock.over > 1) { record(vp, "bridge-dock.no-h-overflow", "FAIL", `dock visible → overflow ${dock.over}px`); await shot(page, vp, "bridge-dock"); await page.evaluate(() => document.getElementById("bridge-dock").classList.add("hidden")); }
  else record(vp, "bridge-dock.no-h-overflow", "PASS");

  await ctx.close();
}

(async () => {
  const pw = findPlaywright();
  if (!pw) {
    console.error("FATAL: playwright-core not found (looked in ~/.npm/_npx/*/node_modules and app/node_modules).");
    console.error("Fix: cd app && npm i -D playwright-core && npx playwright-core install chromium-headless-shell");
    process.exit(2);
  }
  const { chromium } = createRequire(import.meta.url)(path.join(pw.pkg, "index.js"));
  const srv = await startServer();
  const baseUrl = `http://127.0.0.1:${srv.address().port}/index.html`;
  console.log(`# serving ${ROOT} at ${baseUrl}`);
  console.log(`# playwright-core: ${pw.pkg}`);
  console.log(`# chromium: ${pw.exe}`);
  const browser = await chromium.launch({ headless: true, executablePath: pw.exe });
  try {
    for (const v of VIEWPORTS) await auditViewport(browser, baseUrl, v);
  } finally {
    await browser.close();
    srv.close();
  }

  // summary table
  const fails = results.filter((r) => r.status === "FAIL");
  const pad = (s, n) => String(s).padEnd(n);
  console.log("\n== SUMMARY ==");
  console.log(pad("viewport", 11) + pad("check", 30) + "status");
  console.log("-".repeat(50));
  for (const r of results) console.log(pad(r.vp, 11) + pad(r.check, 30) + r.status);
  console.log(`\n${results.length} checks: ${results.filter((r) => r.status === "PASS").length} PASS, ${fails.length} FAIL, ${results.filter((r) => r.status === "SKIP").length} SKIP`);
  if (fails.length) console.log(`screenshots of failing states: ${screensDir}`);
  process.exit(fails.length ? 1 : 0);
})().catch((e) => { console.error("FATAL:", e); process.exit(2); });
