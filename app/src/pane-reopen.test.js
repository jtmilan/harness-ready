// Tests for the PURE pane-reopen idx selection (pane-reopen.js, D63). Selection only;
// the persist→load→reopen RESUME correctness is GUI-verified, not here.
import { describe, expect, it } from "vitest";
import { paneIdx, survivorIdxList } from "./pane-reopen.js";

describe("paneIdx", () => {
  it("parses the -p<idx> suffix", () => {
    expect(paneIdx("ws98119x0-p0")).toBe(0);
    expect(paneIdx("ws98119x0-p2")).toBe(2);
    expect(paneIdx("ws123456x9-p12")).toBe(12);
  });
  it("returns -1 when no suffix / bad input", () => {
    expect(paneIdx("ws98119x0")).toBe(-1);
    expect(paneIdx(null)).toBe(-1);
    expect(paneIdx(undefined)).toBe(-1);
  });
});

describe("survivorIdxList", () => {
  it("after a NON-LAST close, returns the SURVIVORS' own idxs (not 0..count-1)", () => {
    // spawned p0,p1,p2; closed p1 → paneIds compacted to [p0,p2]
    expect(survivorIdxList(["ws1-p0", "ws1-p2"], 2)).toEqual([0, 2]);
  });
  it("no closes → contiguous", () => {
    expect(survivorIdxList(["ws1-p0", "ws1-p1", "ws1-p2"], 3)).toEqual([0, 1, 2]);
  });
  it("closed the LAST pane → [0,1]", () => {
    expect(survivorIdxList(["ws1-p0", "ws1-p1"], 2)).toEqual([0, 1]);
  });
  it("legacy (no surviving ids) → falls back to 0..count-1", () => {
    expect(survivorIdxList([], 3)).toEqual([0, 1, 2]);
    expect(survivorIdxList(undefined, 2)).toEqual([0, 1]);
  });
  it("legacy empty + count 0 → []", () => {
    expect(survivorIdxList([], 0)).toEqual([]);
  });
});
