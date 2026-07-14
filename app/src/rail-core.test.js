// Tests for the PURE rail reconcile core (rail-core.js, 10-01). Set-math + meta only;
// motion correctness (FLIP/fades/pulse/reduced-motion) is GUI-verified, NOT here.
import { describe, expect, it } from "vitest";
import { reconcilePlan, railMeta } from "./rail-core.js";

describe("reconcilePlan", () => {
  it("idempotent: same ids + order → nothing enters/exits/moves (the no-motion tick)", () => {
    const p = reconcilePlan(["a", "b", "c"], [{ id: "a" }, { id: "b" }, { id: "c" }]);
    expect(p.enter).toEqual([]);
    expect(p.exit).toEqual([]);
    expect(p.move).toEqual([]);
    expect(p.reuse).toEqual(["a", "b", "c"]);
    expect(p.nextIds).toEqual(["a", "b", "c"]);
  });

  it("reorder (rank change): every shifted id is a move; no enter/exit", () => {
    const p = reconcilePlan(["a", "b", "c"], [{ id: "c" }, { id: "a" }, { id: "b" }]);
    expect(p.enter).toEqual([]);
    expect(p.exit).toEqual([]);
    expect(p.move).toEqual(["c", "a", "b"]);
    expect(p.nextIds).toEqual(["c", "a", "b"]);
  });

  it("enter + exit + survivor in one tick", () => {
    const p = reconcilePlan(["a", "b"], [{ id: "b" }, { id: "c" }]);
    expect(p.enter).toEqual(["c"]);
    expect(p.exit).toEqual(["a"]);
    expect(p.reuse).toEqual(["b"]);
  });

  it("empty next → everything exits", () => {
    const p = reconcilePlan(["a", "b"], []);
    expect(p.exit).toEqual(["a", "b"]);
    expect(p.enter).toEqual([]);
    expect(p.nextIds).toEqual([]);
  });

  it("empty prev → everything enters (first paint)", () => {
    const p = reconcilePlan([], [{ id: "a" }, { id: "b" }]);
    expect(p.enter).toEqual(["a", "b"]);
    expect(p.exit).toEqual([]);
  });

  it("never re-ranks: nextIds preserves the row order it was given", () => {
    const p = reconcilePlan(["a"], [{ id: "z" }, { id: "a" }, { id: "m" }]);
    expect(p.nextIds).toEqual(["z", "a", "m"]); // as-given, not sorted
  });

  it("tolerates non-array args", () => {
    expect(reconcilePlan(undefined, undefined)).toEqual({
      enter: [], exit: [], reuse: [], move: [], nextIds: [],
    });
  });
});

describe("railMeta", () => {
  it("joins harness + state", () => {
    expect(railMeta({ harness: "claude", state: "working", reason: "-" })).toBe("claude working");
  });
  it("appends reason when present and not '-'", () => {
    expect(railMeta({ harness: "cursor", state: "idle", reason: "asked a question" }))
      .toBe("cursor idle · asked a question");
  });
  it("trims the synthetic 'starting' row (empty harness, no reason)", () => {
    expect(railMeta({ harness: "", state: "starting", reason: "-" })).toBe("starting");
  });
});
