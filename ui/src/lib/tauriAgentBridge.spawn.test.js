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

describe("TauriAgentBridge.spawnAgents — workspace unity (one tenant per workspace)", () => {
  // Regression guard for the bug where every manual NEW AGENT minted its OWN backend
  // ws prefix at `-p0`, so a workspace's panes each looked like a separate workspace
  // to the coordinator's MCP scope (team_get_queue / prompt_all / list_workspaces
  // could not address the fleet as one unit). Reusing a wsId across calls must hand
  // out the next free `-pN` slot — never collide at `-p0`.
  it("reusing a wsId across single-add calls increments the pane index", async () => {
    const { invoke } = installGlobals();
    vi.resetModules();
    ({ TauriAgentBridge } = await import("@/lib/tauriAgentBridge"));
    const bridge = new TauriAgentBridge();
    bridge._poll = vi.fn(async () => {});
    const wsId = "ws55555x0";
    await bridge.spawnAgents([{ kind: "claude-code" }], "T", { assignTo: wsId, wsId });
    await bridge.spawnAgents([{ kind: "cursor" }], "T", { assignTo: wsId, wsId });
    await bridge.spawnAgents([{ kind: "grok" }], "T", { assignTo: wsId, wsId });
    const ids = spawnCalls(invoke).map((c) => c[1].id);
    expect(ids).toEqual(["ws55555x0-p0", "ws55555x0-p1", "ws55555x0-p2"]);
  });

  it("a fresh wsId still starts at -p0 with a shared prefix", async () => {
    const { invoke } = installGlobals();
    vi.resetModules();
    ({ TauriAgentBridge } = await import("@/lib/tauriAgentBridge"));
    const bridge = new TauriAgentBridge();
    bridge._poll = vi.fn(async () => {});
    await bridge.spawnAgents([{ kind: "claude-code" }, { kind: "cursor" }], "T");
    const ids = spawnCalls(invoke).map((c) => c[1].id);
    expect(ids[0]).toMatch(/-p0$/);
    expect(ids[1]).toMatch(/-p1$/);
    expect(ids[0].slice(0, -3)).toBe(ids[1].slice(0, -3)); // same ws prefix
  });
});
