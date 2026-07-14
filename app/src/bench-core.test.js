import { describe, it, expect } from "vitest";
import {
  runDurationMs,
  isGatedVerdict,
  isPassVerdict,
  benchmarkRows,
  fmtDur,
  fmtCost,
} from "./bench-core.js";

// Build a run record. start = run_id "delegate-<ms>"; end = ts_ms → duration = end-start.
const run = (startMs, endMs, over = {}) => [
  `delegate-${startMs}`,
  { ts_ms: endMs, harness: "claude", model: "opus", verdict: "pass", workspace_id: "ws-1", ...over },
];

describe("runDurationMs", () => {
  it("is completion − start", () => {
    expect(runDurationMs("delegate-1000", { ts_ms: 46000 })).toBe(45000);
  });
  it("is 0 when end is missing, non-positive, or before start (excluded from avg)", () => {
    expect(runDurationMs("delegate-1000", { ts_ms: 0 })).toBe(0);
    expect(runDurationMs("delegate-5000", { ts_ms: 1000 })).toBe(0);
    expect(runDurationMs("delegate-1000", null)).toBe(0);
    expect(runDurationMs("not-a-run", { ts_ms: 9 })).toBe(0);
  });
  it("prefers stored duration_ms over the run_id parse", () => {
    // stored overall wall-clock wins outright
    expect(runDurationMs("delegate-1000", { ts_ms: 999999, duration_ms: 5000 })).toBe(5000);
    // a panes-runner date-string run_id would parseInt to a tiny year (ts_ms-as-duration bug);
    // duration_ms rescues it
    expect(runDurationMs("2026-06-23-234424", { ts_ms: 1787000000000, duration_ms: 360000 })).toBe(360000);
    // duration_ms 0/absent → fall back to the legacy parse (headless + old records stay correct)
    expect(runDurationMs("delegate-1000", { ts_ms: 46000, duration_ms: 0 })).toBe(45000);
  });
});

describe("verdict classification", () => {
  it("gates only pass/reject; pass-only is a pass", () => {
    expect(isGatedVerdict("pass")).toBe(true);
    expect(isGatedVerdict("REJECT")).toBe(true);
    expect(isGatedVerdict("advisory")).toBe(false);
    expect(isGatedVerdict("hold")).toBe(false);
    expect(isGatedVerdict("")).toBe(false);
    expect(isPassVerdict("pass")).toBe(true);
    expect(isPassVerdict("reject")).toBe(false);
  });
});

describe("benchmarkRows", () => {
  it("groups by harness·model and averages only timed/costed runs", () => {
    const entries = [
      run(1000, 31000, { usage: { cost_usd: 0.10 } }), // claude·opus 30s $0.10 pass
      run(1000, 61000, { usage: { cost_usd: 0.30 } }), // claude·opus 60s $0.30 pass
      run(1000, 0, { usage: { cost_usd: 0.20 } }),     // claude·opus untimed (dur 0) — excluded from avgDur, still a run
    ];
    const rows = benchmarkRows(entries);
    expect(rows).toHaveLength(1);
    const r = rows[0];
    expect(r.harness).toBe("claude");
    expect(r.model).toBe("opus");
    expect(r.runs).toBe(3);
    expect(r.timedRuns).toBe(2);
    expect(r.avgDurMs).toBe(45000); // (30s+60s)/2 — the untimed run does NOT pull it to 30s
    expect(r.costRuns).toBe(3);
    expect(r.avgCostUsd).toBeCloseTo(0.2, 6); // (0.1+0.3+0.2)/3
    expect(r.passRate).toBe(1); // 3 gated, 3 pass
  });

  it("sorts FASTEST-first; a model with no timed run sinks to the bottom", () => {
    const entries = [
      run(1000, 121000, { model: "slow" }),                 // 120s
      run(1000, 11000, { model: "fast" }),                  // 10s
      run(1000, 0, { model: "untimed" }),                   // no measurable span
    ];
    const rows = benchmarkRows(entries);
    expect(rows.map((r) => r.model)).toEqual(["fast", "slow", "untimed"]);
  });

  it("scopes by workspace id", () => {
    const entries = [
      run(1000, 11000, { workspace_id: "ws-1", model: "a" }),
      run(1000, 11000, { workspace_id: "ws-2", model: "b" }),
    ];
    expect(benchmarkRows(entries, "ws-1").map((r) => r.model)).toEqual(["a"]);
    expect(benchmarkRows(entries, "__all__")).toHaveLength(2);
  });

  it("pass-rate counts only gated runs; advisory/hold excluded; null when none gated", () => {
    const entries = [
      run(1000, 11000, { verdict: "pass" }),
      run(1000, 11000, { verdict: "reject" }),
      run(1000, 11000, { verdict: "advisory" }), // not gated
    ];
    const r = benchmarkRows(entries)[0];
    expect(r.runs).toBe(3);
    expect(r.gatedRuns).toBe(2);
    expect(r.passes).toBe(1);
    expect(r.passRate).toBe(0.5); // 1 pass / 2 gated — advisory excluded

    const allAdvisory = benchmarkRows([run(1000, 11000, { verdict: "advisory" })])[0];
    expect(allAdvisory.passRate).toBeNull();
  });

  it("defaults missing harness/model and tolerates an empty/garbage list", () => {
    expect(benchmarkRows([])).toEqual([]);
    const r = benchmarkRows([["delegate-1", { ts_ms: 2 }]])[0];
    expect(r.harness).toBe("—");
    expect(r.model).toBe("default");
  });

  it("excludes multi-harness (joined) panes runs from the per-harness rollup", () => {
    const entries = [
      run(1000, 31000), // claude·opus single-harness — kept
      [
        "2026-06-23-234424",
        {
          ts_ms: 1787000000000,
          duration_ms: 360000,
          harness: "claude+cursor+commandcode", // joined → not attributable to one harness
          model: "opus+auto+glm",
          verdict: "pass",
          workspace_id: "ws-1",
        },
      ],
    ];
    const rows = benchmarkRows(entries);
    expect(rows).toHaveLength(1);
    expect(rows[0].harness).toBe("claude");
    expect(rows.some((r) => String(r.harness).includes("+"))).toBe(false);
  });
});

describe("formatters", () => {
  it("fmtDur", () => {
    expect(fmtDur(0)).toBe("—");
    expect(fmtDur(45000)).toBe("45s");
    expect(fmtDur(125000)).toBe("2m 5s");
    expect(fmtDur(180000)).toBe("3m");
  });
  it("fmtCost", () => {
    expect(fmtCost(0, 0)).toBe("—");
    expect(fmtCost(0.0042, 1)).toBe("$0.0042");
    expect(fmtCost(1.235, 2)).toBe("$1.24");
  });
});
