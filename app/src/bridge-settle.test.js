// 07-T3: Unit tests for the Bridge two-wave settle predicate (`isPaneSettled`).
//
// The core regression guard: a code-wave pane with "## BOUNDARIES" in its report but
// NO commit on its branch must NOT be counted as settled. If it were, assembleAndDispatch
// Verify() would fire immediately, the assembler would fold zero branches, and the verify
// wave would run against an empty integration tree — a silent false-pass.
import { describe, it, expect } from "vitest";
import { isPaneSettled } from "./bridge-settle.js";

// Helpers ----------------------------------------------------------------

/** Build a minimal PaneReady row (all false / 0 defaults, override per test). */
const row = (over) => ({
  id: "p0",
  bytes: 0,
  complete: false,
  dead: false,
  committed: false,
  ...over,
});

// -----------------------------------------------------------------------

describe("isPaneSettled — single-wave / verify-wave pane (isCodeWave=false)", () => {
  it("not settled: no bytes yet", () => {
    expect(isPaneSettled(row({ bytes: 0 }), 0, false)).toBe(false);
  });

  it("not settled: bytes present but no BOUNDARIES sentinel", () => {
    expect(isPaneSettled(row({ bytes: 500, complete: false }), 500, false)).toBe(false);
  });

  it("not settled: BOUNDARIES present but still growing (unstable)", () => {
    // previous bytes was 800; current is 1000 → still writing
    expect(isPaneSettled(row({ bytes: 1000, complete: true }), 800, false)).toBe(false);
  });

  it("settled: BOUNDARIES + stable size (two consecutive polls agree)", () => {
    expect(isPaneSettled(row({ bytes: 1200, complete: true }), 1200, false)).toBe(true);
  });

  it("committed field is ignored for non-code-wave panes (verify panes never commit)", () => {
    // committed=false must not block settle for a verify-wave pane
    expect(isPaneSettled(row({ bytes: 900, complete: true, committed: false }), 900, false)).toBe(true);
  });
});

describe("isPaneSettled — code-wave pane (isCodeWave=true) — 07-T3 commit gate", () => {
  it("THE REGRESSION CASE: BOUNDARIES present + stable + committed=false → NOT settled", () => {
    // This is the exact race: the agent wrote the report (complete+stable) but has not
    // yet run `git commit`. The assembler must NOT fire until committed=true.
    expect(isPaneSettled(row({ bytes: 1500, complete: true, committed: false }), 1500, true)).toBe(false);
  });

  it("not settled: no bytes (no report at all)", () => {
    expect(isPaneSettled(row({ bytes: 0, committed: false }), 0, true)).toBe(false);
  });

  it("not settled: BOUNDARIES present + committed=true but size is still growing", () => {
    expect(isPaneSettled(row({ bytes: 2000, complete: true, committed: true }), 1500, true)).toBe(false);
  });

  it("not settled: size stable + committed=true but no BOUNDARIES yet", () => {
    expect(isPaneSettled(row({ bytes: 700, complete: false, committed: true }), 700, true)).toBe(false);
  });

  it("settled: BOUNDARIES + stable + committed=true (all three gates pass)", () => {
    expect(isPaneSettled(row({ bytes: 1800, complete: true, committed: true }), 1800, true)).toBe(true);
  });
});

describe("isPaneSettled — dead panes (commit gate is bypassed)", () => {
  it("dead pane with no bytes → settled (it will never write)", () => {
    expect(isPaneSettled(row({ dead: true, bytes: 0, committed: false }), 0, true)).toBe(true);
    expect(isPaneSettled(row({ dead: true, bytes: 0, committed: false }), 0, false)).toBe(true);
  });

  it("dead pane with stable partial output → settled (it stopped mid-write, PTY exited)", () => {
    // A dead code-wave pane that wrote a partial report but never committed: the commit
    // gate is NOT applied — the pane cannot recover, so we accept the partial output and
    // let synthesis classify it as 'incomplete'.
    expect(isPaneSettled(row({ dead: true, bytes: 300, complete: false, committed: false }), 300, true)).toBe(true);
  });

  it("dead pane still growing (unusual but possible with buffered output) → NOT settled", () => {
    expect(isPaneSettled(row({ dead: true, bytes: 400, complete: false }), 300, true)).toBe(false);
  });
});
