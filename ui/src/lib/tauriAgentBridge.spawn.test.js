// K1 + K4 contracts for TauriAgentBridge.spawnAgents.
// Mocks window.__TAURI__.core.invoke at the IO boundary — no real backend.
import { beforeEach, describe, it, expect, vi } from "vitest";

function installGlobals() {
  const store = new Map();
  const localStorage = {
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
  globalThis.localStorage = localStorage;
  const invoke = vi.fn(async (cmd) => {
    if (cmd === "list_queue") return [];
    if (cmd === "dead_pane_ids") return [];
    if (cmd === "read_output_delta_batch") return [];
    if (cmd === "spawn_workspace") return undefined;
    if (cmd === "set_pane_roles") return undefined;
    return undefined;
  });
  globalThis.window = {
    __TAURI__: { core: { invoke } },
    addEventListener: () => {},
    removeEventListener: () => {},
    localStorage,
  };
  return { invoke, store };
}

/** @type {typeof import("./tauriAgentBridge.js").TauriAgentBridge} */
let TauriAgentBridge;

beforeEach(async () => {
  installGlobals();
  vi.resetModules();
  ({ TauriAgentBridge } = await import("@/lib/tauriAgentBridge"));
});

function spawnCalls(invoke) {
  return invoke.mock.calls.filter((c) => c[0] === "spawn_workspace");
}

describe("TauriAgentBridge.spawnAgents — mapped harness wire", () => {
  // Live today: known kinds must not be rewritten to bash. Complements K4 refusal.
  it("mapped kinds pass their wire string to spawn_workspace", async () => {
    const { invoke } = installGlobals();
    vi.resetModules();
    ({ TauriAgentBridge } = await import("@/lib/tauriAgentBridge"));
    const bridge = new TauriAgentBridge();
    bridge._poll = vi.fn(async () => {});

    await bridge.spawnAgents(
      [
        { kind: "claude-code" },
        { kind: "cursor" },
        { kind: "grok" },
        { kind: "bash" },
      ],
      "MAPPED",
    );

    const harnesses = spawnCalls(invoke).map((c) => c[1].harness);
    expect(harnesses).toEqual(["claude", "cursor", "grok", "bash"]);
  });
});

describe("TauriAgentBridge.spawnAgents — K4 refusal (pinned; p4)", () => {
  // Base still has `HARNESS_WIRE[cfg.kind] || "bash"` — full refusal is p4's job.
  // Keep the suite green via todo; do not assert the broken fallback.
  it.todo(
    "unmapped kind is refused — no spawn_workspace, no ghost spawned entry, red raw + console.error (never harness bash)",
  );
});

describe("TauriAgentBridge.spawnAgents — K1 return value (pinned; p4)", () => {
  it.todo(
    "resolves to Promise<string[]> of minted ids in config order",
  );
  it.todo(
    "failed spawn_workspace invoke is excluded from the returned id list",
  );
});
