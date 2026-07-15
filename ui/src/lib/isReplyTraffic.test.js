// Guards BUG-2's broadcast fix against over-filtering: reply traffic stays
// single-pane; real keystrokes (incl. arrows / shift+tab) still fan out.
// Pure regex over a string — no DOM, no Tauri.
import { beforeAll, describe, it, expect } from "vitest";

/** Minimal browser globals so paneLabels (imported via tauriAgentBridge) can eval. */
function installBrowserStubs() {
  const store = new Map();
  globalThis.localStorage = {
    getItem: (k) => (store.has(k) ? store.get(k) : null),
    setItem: (k, v) => {
      store.set(String(k), String(v));
    },
    removeItem: (k) => {
      store.delete(String(k));
    },
    clear: () => {
      store.clear();
    },
  };
  globalThis.window = {
    addEventListener: () => {},
    removeEventListener: () => {},
    localStorage: globalThis.localStorage,
  };
}

/** @type {typeof import("./tauriAgentBridge.js").isReplyTraffic} */
let isReplyTraffic;

beforeAll(async () => {
  installBrowserStubs();
  ({ isReplyTraffic } = await import("@/lib/tauriAgentBridge"));
});

describe("isReplyTraffic", () => {
  it("SGR mouse reports route true", () => {
    expect(isReplyTraffic("\x1b[<35;28;24M")).toBe(true);
    expect(isReplyTraffic("\x1b[<0;12;34m")).toBe(true);
  });

  it("focus in/out reports route true", () => {
    expect(isReplyTraffic("\x1b[I")).toBe(true);
    expect(isReplyTraffic("\x1b[O")).toBe(true);
  });

  it("OSC query replies route true", () => {
    expect(isReplyTraffic("\x1b]11;rgb:0a0a/0b0b/0c0c")).toBe(true);
    expect(isReplyTraffic("\x1b]")).toBe(true);
  });

  it("arrow keys route false (must still broadcast)", () => {
    expect(isReplyTraffic("\x1b[A")).toBe(false); // up
    expect(isReplyTraffic("\x1b[B")).toBe(false); // down
    expect(isReplyTraffic("\x1b[C")).toBe(false); // right
    expect(isReplyTraffic("\x1b[D")).toBe(false); // left
  });

  it("shift+tab (\\x1b[Z) routes false", () => {
    expect(isReplyTraffic("\x1b[Z")).toBe(false);
  });

  it("plain text and ordinary keystrokes route false", () => {
    expect(isReplyTraffic("hello")).toBe(false);
    expect(isReplyTraffic("a")).toBe(false);
    expect(isReplyTraffic("\r")).toBe(false);
    expect(isReplyTraffic("\t")).toBe(false);
    expect(isReplyTraffic("")).toBe(false);
  });
});
