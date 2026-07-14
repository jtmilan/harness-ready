// board-core unit tests — pure logic, node env (no DOM). Covers the columnFor mapping,
// boardRows shaping (queue ∪ workspace extras, orphan guard, rank-order preservation),
// bucketByColumn (order preservation + unknown-column fallback), and applyOrder
// (session-local override on top of rank order).
import { describe, it, expect } from "vitest";
import { BOARD_COLS, columnFor, boardRows, bucketByColumn, applyOrder, boardReorder } from "./board-core.js";

describe("columnFor", () => {
  it("needs_human wins over any state", () => {
    expect(columnFor({ state: "working", needs_human: true })).toBe("Needs you");
    expect(columnFor({ state: "error", needs_human: true })).toBe("Needs you");
  });
  it("maps the four terminal/live states", () => {
    expect(columnFor({ state: "working" })).toBe("Working");
    expect(columnFor({ state: "idle" })).toBe("Idle");
    expect(columnFor({ state: "done" })).toBe("Done");
    expect(columnFor({ state: "error" })).toBe("Error");
  });
  it("rate_limit reason → Scheduled (D33: scheduler-owned, not Needs you)", () => {
    expect(columnFor({ state: "queued", reason: "rate_limit" })).toBe("Scheduled");
  });
  it("is total: unknown state / null / undefined fall to Starting, never throws", () => {
    expect(columnFor({ state: "starting" })).toBe("Starting");
    expect(columnFor({ state: "???" })).toBe("Starting");
    expect(columnFor({})).toBe("Starting");
    expect(columnFor(null)).toBe("Starting");
    expect(columnFor(undefined)).toBe("Starting");
  });
  it("never maps a workspace row to Backlog (tasks-only lane)", () => {
    for (const row of [{ state: "working" }, { state: "idle" }, {}, { needs_human: true }]) {
      expect(columnFor(row)).not.toBe("Backlog");
    }
  });
});

describe("boardRows", () => {
  const workspaces = { w1: { paneIds: ["w1-p0", "w1-p1"] }, w2: { paneIds: ["w2-p0"] } };
  const paneOwner = (id) => {
    for (const wsId of Object.keys(workspaces)) {
      if (workspaces[wsId].paneIds.includes(id)) return wsId;
    }
    return null;
  };

  it("preserves list_queue order first, then appends workspace extras as synthetic starting rows", () => {
    const queue = [{ id: "w1-p1", state: "working" }, { id: "w2-p0", state: "idle" }];
    const all = ["w1-p0", "w1-p1", "w2-p0"];
    const rows = boardRows(queue, all, workspaces, paneOwner);
    expect(rows.map((r) => r.id)).toEqual(["w1-p1", "w2-p0", "w1-p0"]);
    const extra = rows[2];
    expect(extra).toEqual({ id: "w1-p0", harness: "", state: "starting", reason: "-", needs_human: false });
  });
  it("drops orphan rows with no owning workspace (phantom guard)", () => {
    const queue = [{ id: "ghost-p0", state: "working" }, { id: "w1-p0", state: "working" }];
    const rows = boardRows(queue, ["also-ghost"], workspaces, paneOwner);
    expect(rows.map((r) => r.id)).toEqual(["w1-p0"]);
  });
  it("tolerates null/absent queue and all", () => {
    expect(boardRows(null, null, workspaces, paneOwner)).toEqual([]);
    expect(boardRows(undefined, ["w2-p0"], workspaces, paneOwner).map((r) => r.id)).toEqual(["w2-p0"]);
  });
});

describe("bucketByColumn", () => {
  it("buckets in array order per column (in-column order == list_queue order, no sort)", () => {
    const rows = [
      { id: "a", state: "working" },
      { id: "b", needs_human: true },
      { id: "c", state: "working" },
      { id: "d", state: "idle" },
    ];
    const buckets = bucketByColumn(rows, BOARD_COLS);
    expect(buckets["Working"]).toEqual(["a", "c"]);
    expect(buckets["Needs you"]).toEqual(["b"]);
    expect(buckets["Idle"]).toEqual(["d"]);
    expect(buckets["Backlog"]).toEqual([]);
  });
  it("a column name not in cols falls to the LAST col", () => {
    // cols without "Working" → a working row lands in the final bucket.
    const buckets = bucketByColumn([{ id: "a", state: "working" }], ["Idle", "Starting"]);
    expect(buckets["Starting"]).toEqual(["a"]);
    expect(buckets["Idle"]).toEqual([]);
  });
  it("handles empty/null rows", () => {
    const buckets = bucketByColumn(null, ["X"]);
    expect(buckets).toEqual({ X: [] });
  });
});

describe("applyOrder", () => {
  it("no override → rank order, as a copy (never the same array)", () => {
    const bucket = ["a", "b", "c"];
    const out = applyOrder(bucket, null);
    expect(out).toEqual(["a", "b", "c"]);
    expect(out).not.toBe(bucket);
    expect(applyOrder(bucket, [])).toEqual(["a", "b", "c"]);
  });
  it("override ids lead in override order; the rest keep rank order", () => {
    expect(applyOrder(["a", "b", "c", "d"], ["c", "a"])).toEqual(["c", "a", "b", "d"]);
  });
  it("override ids that left the bucket are ignored", () => {
    expect(applyOrder(["a", "b"], ["gone", "b"])).toEqual(["b", "a"]);
  });
});

describe("boardReorder", () => {
  it("cross-column drop returns null (Model A no-op)", () => {
    expect(boardReorder(["a", "b"], "Working", "Idle", "a", null)).toBeNull();
  });
  it("intra-column: drop before a target id", () => {
    expect(boardReorder(["a", "b", "c"], "Working", "Working", "c", "a")).toEqual(["c", "a", "b"]);
  });
  it("intra-column: no beforeId (or a vanished one) appends to the end", () => {
    expect(boardReorder(["a", "b", "c"], "Working", "Working", "a", null)).toEqual(["b", "c", "a"]);
    expect(boardReorder(["a", "b", "c"], "Working", "Working", "a", "zzz")).toEqual(["b", "c", "a"]);
  });
});
