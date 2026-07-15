// K5 contract + default-bucket regression. Node env; localStorage stubbed.
// Module has a top-level `let assignment = load()` cache, so each test gets a
// FRESH module instance: vi.resetModules() + dynamic import in beforeEach.
import { beforeEach, describe, it, expect, vi } from "vitest";

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
  // Fresh instance: load() runs against the stub above via getItem only, so
  // setItem call-count assertions below start at zero regardless.
  workspaceAssign = await import("@/lib/workspaceAssign");
});

describe("paneIdsForWorkspace — default-bucket fallback (unchanged)", () => {
  it("unassigned panes appear only under defaultWsId", () => {
    const all = ["p0", "p1", "p2"];
    expect(workspaceAssign.paneIdsForWorkspace("ws-a", all, "ws-a")).toEqual([
      "p0",
      "p1",
      "p2",
    ]);
    expect(workspaceAssign.paneIdsForWorkspace("ws-b", all, "ws-a")).toEqual([]);
  });

  it("explicit assign moves a pane out of the default bucket", () => {
    workspaceAssign.assign("p1", "ws-b");
    const all = ["p0", "p1", "p2"];
    expect(workspaceAssign.paneIdsForWorkspace("ws-a", all, "ws-a")).toEqual([
      "p0",
      "p2",
    ]);
    expect(workspaceAssign.paneIdsForWorkspace("ws-b", all, "ws-a")).toEqual([
      "p1",
    ]);
  });

  it("null defaultWsId hides unassigned panes", () => {
    const all = ["p0", "p1"];
    expect(workspaceAssign.paneIdsForWorkspace("ws-a", all, null)).toEqual([]);
    workspaceAssign.assign("p0", "ws-a");
    expect(workspaceAssign.paneIdsForWorkspace("ws-a", all, null)).toEqual([
      "p0",
    ]);
  });
});

// K5 — assignMany. p5 owns the implementation. Activate bodies when export lands
// (typeof check); otherwise it.todo keeps the gate green without soft asserts.
// Probe import at collection time (export presence is static); beforeEach's
// resetModules still gives each test its own fresh instance.
const assignManyReady =
  typeof (await import("@/lib/workspaceAssign")).assignMany === "function";

describe("assignMany (K5)", () => {
  (assignManyReady ? it : it.todo)(
    "N ids = exactly one localStorage write",
    () => {
      const { api } = installLocalStorage();
      workspaceAssign.assignMany(["a", "b", "c"], "ws-team");
      expect(api.setItem).toHaveBeenCalledTimes(1);
      expect(workspaceAssign.getAssignment()).toEqual({
        a: "ws-team",
        b: "ws-team",
        c: "ws-team",
      });
    },
  );

  (assignManyReady ? it : it.todo)(
    "empty list = no write",
    () => {
      const { api } = installLocalStorage();
      workspaceAssign.assignMany([], "ws-team");
      expect(api.setItem).not.toHaveBeenCalled();
      expect(workspaceAssign.getAssignment()).toEqual({});
    },
  );

  (assignManyReady ? it : it.todo)(
    "assigned ids leave the default bucket for paneIdsForWorkspace",
    () => {
      installLocalStorage();
      workspaceAssign.assignMany(["x", "y"], "ws-b");
      const all = ["x", "y", "z"];
      expect(workspaceAssign.paneIdsForWorkspace("ws-a", all, "ws-a")).toEqual([
        "z",
      ]);
      expect(workspaceAssign.paneIdsForWorkspace("ws-b", all, "ws-a")).toEqual([
        "x",
        "y",
      ]);
    },
  );
});
