import { describe, it, expect } from "vitest";
import { allLivePanes } from "./bridge-panes.js";

const ws = (over) => ({ harness: "claude", paneIds: [], harnesses: [], roles: [], dormant: false, repo: "/r", ...over });

describe("allLivePanes (UNIFY cross-workspace)", () => {
  it("returns [] with no active workspace", () => {
    expect(allLivePanes({ workspaces: { a: ws() }, sessions: {}, deadPanes: new Set(), activeWs: null })).toEqual([]);
  });

  it("aggregates live panes across workspaces sharing the active repo", () => {
    const workspaces = {
      a: ws({ repo: "/r", paneIds: ["a-p0"], harnesses: ["claude"], roles: ["scout"] }),
      b: ws({ repo: "/r", paneIds: ["b-p0", "b-p1"], harnesses: ["codex", "cursor"], roles: [null, "tester"] }),
    };
    const sessions = { "a-p0": 1, "b-p0": 1, "b-p1": 1 };
    const out = allLivePanes({ workspaces, sessions, deadPanes: new Set(), activeWs: "a" });
    expect(out.map((p) => p.id).sort()).toEqual(["a-p0", "b-p0", "b-p1"]);
    expect(out.find((p) => p.id === "a-p0").role).toBe("scout");
    expect(out.find((p) => p.id === "b-p1").harness).toBe("cursor");
    expect(out.find((p) => p.id === "b-p0").role).toBeNull();
  });

  it("drops workspaces on a different repo (single-repo orchestrate guard)", () => {
    const workspaces = {
      a: ws({ repo: "/r", paneIds: ["a-p0"], harnesses: ["claude"] }),
      b: ws({ repo: "/other", paneIds: ["b-p0"], harnesses: ["codex"] }),
    };
    const sessions = { "a-p0": 1, "b-p0": 1 };
    const out = allLivePanes({ workspaces, sessions, deadPanes: new Set(), activeWs: "a" });
    expect(out.map((p) => p.id)).toEqual(["a-p0"]);
  });

  it("excludes dormant workspaces, dead panes, and panes with no session", () => {
    const workspaces = {
      a: ws({ repo: "/r", paneIds: ["a-p0", "a-p1"], harnesses: ["claude", "claude"] }),
      b: ws({ repo: "/r", dormant: true, paneIds: ["b-p0"], harnesses: ["codex"] }),
    };
    const sessions = { "a-p0": 1, "a-p1": 1, "b-p0": 1 }; // a-p1 will be dead; no session check for others
    const deadPanes = new Set(["a-p1"]);
    const out = allLivePanes({ workspaces, sessions, deadPanes, activeWs: "a" });
    expect(out.map((p) => p.id)).toEqual(["a-p0"]);
  });

  it("accepts a plain-object deadPanes map (not just a Set)", () => {
    const workspaces = { a: ws({ repo: "/r", paneIds: ["a-p0", "a-p1"], harnesses: ["claude", "claude"] }) };
    const sessions = { "a-p0": 1, "a-p1": 1 };
    const out = allLivePanes({ workspaces, sessions, deadPanes: { "a-p0": true }, activeWs: "a" });
    expect(out.map((p) => p.id)).toEqual(["a-p1"]);
  });
});
