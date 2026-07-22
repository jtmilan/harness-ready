// workspaceStore — resetWorkspaces + launchPadWorkspaces contract.
// Node env (vitest config environment: "node"); localStorage stubbed.
// workspaceAssign holds a top-level `let assignment = load()` cache, so each
// test gets a FRESH module instance: vi.resetModules() + dynamic import in
// beforeEach (same pattern as workspaceAssign.test.js — required so the store
// and the assignment map both re-read the freshly shimmed localStorage).
import { beforeEach, describe, it, expect, vi } from "vitest";

/** @type {typeof import("@/lib/workspaceStore")} re-imported fresh per test */
let workspaceStore;
/** @type {typeof import("@/lib/workspaceAssign")} re-imported fresh per test */
let workspaceAssign;

function installLocalStorage() {
  const store = new Map();
  const api = {
    getItem: vi.fn((k) => (store.has(k) ? store.get(k) : null)),
    setItem: vi.fn((k, v) => {
      store.set(String(k), String(v));
    }),
    removeItem: vi.fn((k) => {
      store.delete(String(k));
    }),
    clear: vi.fn(() => {
      store.clear();
    }),
  };
  globalThis.localStorage = api;
  return { store, api };
}

beforeEach(async () => {
  installLocalStorage();
  vi.resetModules();
  // Fresh instances: workspaceStore's static import of workspaceAssign resolves
  // to the same cached module the second dynamic import returns, so both share
  // one assignment map. load() runs against the stub above via getItem.
  workspaceStore = await import("@/lib/workspaceStore");
  workspaceAssign = await import("@/lib/workspaceAssign");
});

describe("launchPadWorkspaces — seed source of truth", () => {
  it("returns the single default workspace", () => {
    expect(workspaceStore.launchPadWorkspaces()).toEqual([
      { id: "ws-1", name: "MY WORKSPACE" },
    ]);
  });

  it("returns a fresh array each call (callers cannot mutate the seed)", () => {
    const a = workspaceStore.launchPadWorkspaces();
    const b = workspaceStore.launchPadWorkspaces();
    expect(a).toEqual(b);
    expect(a).not.toBe(b);
  });
});

describe("resetWorkspaces — launch-pad return", () => {
  it("resets custom workspaces + assignments to the launch-pad seed", () => {
    // seed: two custom workspaces, two pane assignments pointing at them
    workspaceStore.saveWorkspaces([
      { id: "ws-custom", name: "CUSTOM 1" },
      { id: "ws-custom2", name: "CUSTOM 2" },
    ]);
    workspaceAssign.assign("pane-a", "ws-custom");
    workspaceAssign.assign("pane-b", "ws-custom2");

    const result = workspaceStore.resetWorkspaces();

    // returns the launch-pad seed
    expect(result).toEqual([{ id: "ws-1", name: "MY WORKSPACE" }]);
    // loadWorkspaces re-reads the seed from localStorage
    expect(workspaceStore.loadWorkspaces()).toEqual([
      { id: "ws-1", name: "MY WORKSPACE" },
    ]);
    // every pane assignment is scrubbed
    expect(workspaceAssign.getAssignment()).toEqual({});
  });

  it("is idempotent — calling twice yields the same seed with empty assignments", () => {
    workspaceStore.saveWorkspaces([{ id: "ws-x", name: "X" }]);
    workspaceAssign.assign("p1", "ws-x");

    workspaceStore.resetWorkspaces();
    const second = workspaceStore.resetWorkspaces();

    expect(second).toEqual([{ id: "ws-1", name: "MY WORKSPACE" }]);
    expect(workspaceStore.loadWorkspaces()).toEqual([
      { id: "ws-1", name: "MY WORKSPACE" },
    ]);
    expect(workspaceAssign.getAssignment()).toEqual({});
  });

  it("returned seed is a fresh array — mutating it does not poison loadWorkspaces", () => {
    const a = workspaceStore.resetWorkspaces();
    // mutate the returned array and its inner object
    a.push({ id: "ws-evil", name: "EVIL" });
    a[0].name = "MUTATED";

    // loadWorkspaces re-parses from localStorage (the pre-mutation seed was persisted)
    expect(workspaceStore.loadWorkspaces()).toEqual([
      { id: "ws-1", name: "MY WORKSPACE" },
    ]);
  });

  it("scrubs assignments pointing at the launch-pad seed ws id too", () => {
    // Even if an assignment points at "ws-1" (the seed id that survives the
    // reset), it must be cleared — the fleet is dead, so any stale entry
    // would orphan into the new launch pad's default bucket.
    workspaceAssign.assign("stale-pane", "ws-1");
    workspaceStore.resetWorkspaces();
    expect(workspaceAssign.getAssignment()).toEqual({});
  });
});
