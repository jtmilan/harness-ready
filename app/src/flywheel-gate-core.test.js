// flywheel-gate-core unit tests — pure copy/decision, node env (no DOM). Pins the 5 gate
// branches of flywheelPhaseCopy + the presence-gated fwGateChips.
import { describe, it, expect } from "vitest";
import { flywheelPhaseCopy, fwGateChips } from "./flywheel-gate-core.js";

describe("flywheelPhaseCopy — gate-driven subtitle", () => {
  it("no gate / not live → observe-only copy", () => {
    const s = flywheelPhaseCopy(null);
    expect(s).toContain("only observes");
    expect(s).toContain("delegate-live build");
    // undefined and an explicit delegate_live:false collapse to the same branch
    expect(flywheelPhaseCopy(undefined)).toBe(s);
    expect(flywheelPhaseCopy({ delegate_live: false, flywheel_apply: true, flywheel_ship: true })).toBe(s);
  });

  it("live + apply explicitly off → report-only copy", () => {
    const s = flywheelPhaseCopy({ delegate_live: true, flywheel_apply: false, flywheel_ship: false });
    expect(s).toContain("Read-only cycle");
    expect(s).toContain("No code is changed");
    expect(s).toContain("open the PR yourself");
  });

  it("live + apply on + ship off → applies-but-stops-before-push copy", () => {
    const s = flywheelPhaseCopy({ delegate_live: true, flywheel_apply: true, flywheel_ship: false });
    expect(s).toContain("applies the fix to the merged tree");
    expect(s).toContain("stops before pushing");
  });

  it("live + apply on + ship on → full-loop auto-PR copy", () => {
    const s = flywheelPhaseCopy({ delegate_live: true, flywheel_apply: true, flywheel_ship: true });
    expect(s).toContain("Full loop");
    expect(s).toContain("auto-opens a PR");
    expect(s).toContain("you merge");
  });

  it("live but apply/ship undefined (older backend) → falls back to the report-only copy", () => {
    const reportOnly = flywheelPhaseCopy({ delegate_live: true, flywheel_apply: false, flywheel_ship: false });
    expect(flywheelPhaseCopy({ delegate_live: true })).toBe(reportOnly);
    // ship present but apply absent still counts as "older backend can't tell us" → report-only
    expect(flywheelPhaseCopy({ delegate_live: true, flywheel_ship: true })).toBe(reportOnly);
  });
});

describe("fwGateChips — presence-gated apply/ship flags", () => {
  it("no gate → empty string", () => {
    expect(fwGateChips(null)).toBe("");
    expect(fwGateChips(undefined)).toBe("");
  });
  it("absent flags render nothing (backward-compat)", () => {
    expect(fwGateChips({ delegate_live: true })).toBe("");
  });
  it("present flags render ✓/✗ chips", () => {
    expect(fwGateChips({ flywheel_apply: true, flywheel_ship: false }))
      .toBe("   ·   apply ✓ · ship ✗");
    expect(fwGateChips({ flywheel_apply: false })).toBe("   ·   apply ✗");
  });
});
