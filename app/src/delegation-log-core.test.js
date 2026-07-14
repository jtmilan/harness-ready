import { describe, it, expect, vi, afterEach } from "vitest";
import {
  DG_MAX_LINES,
  dgRunTs,
  dgRelTime,
  dgDurationMs,
  dgFmtDur,
  dgVerdictUx,
  dgReviewData,
  dgPush,
  dgIngestLine,
} from "./delegation-log-core.js";

afterEach(() => vi.useRealTimers());

describe("dgRunTs", () => {
  it("prefers the record ts_ms", () => {
    expect(dgRunTs("delegate-100", { ts_ms: 999 })).toBe(999);
  });
  it("falls back to parsing the delegate-<ms> run_id", () => {
    expect(dgRunTs("delegate-1700000000000", null)).toBe(1700000000000);
  });
  it("returns 0 for a non-numeric / panes-date run_id with no ts", () => {
    expect(dgRunTs("panes-2026-06-26", null)).toBe(0);
    expect(dgRunTs("", undefined)).toBe(0);
  });
});

describe("dgRelTime", () => {
  it("returns empty for falsy ts", () => {
    expect(dgRelTime(0)).toBe("");
    expect(dgRelTime(null)).toBe("");
  });
  it("buckets seconds/minutes/hours/days", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-07-03T00:00:00Z"));
    const now = Date.now();
    expect(dgRelTime(now - 10 * 1000)).toBe("just now");
    expect(dgRelTime(now - 5 * 60 * 1000)).toBe("5m ago");
    expect(dgRelTime(now - 3 * 3600 * 1000)).toBe("3h ago");
    expect(dgRelTime(now - 2 * 86400 * 1000)).toBe("2d ago");
  });
  it("falls back to a locale date beyond a week", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-07-03T00:00:00Z"));
    const out = dgRelTime(Date.now() - 30 * 86400 * 1000);
    expect(out).not.toMatch(/ago|just now/);
    expect(out.length).toBeGreaterThan(0);
  });
});

describe("dgDurationMs", () => {
  it("prefers the backend duration_ms", () => {
    expect(dgDurationMs("delegate-1000", { duration_ms: 4200, ts_ms: 9999 })).toBe(4200);
  });
  it("falls back to ts_ms − start(run_id)", () => {
    expect(dgDurationMs("delegate-1000", { ts_ms: 6000 })).toBe(5000);
  });
  it("returns 0 when end is not after start (guards the 29784739m bug)", () => {
    expect(dgDurationMs("delegate-9000", { ts_ms: 1000 })).toBe(0);
    expect(dgDurationMs("panes-2026", { ts_ms: 6000 })).toBe(0); // start unparseable
  });
});

describe("dgFmtDur", () => {
  it("returns empty for 0 / negative", () => {
    expect(dgFmtDur(0)).toBe("");
    expect(dgFmtDur(-5)).toBe("");
  });
  it("formats seconds and minutes", () => {
    expect(dgFmtDur(45000)).toBe("45s");
    expect(dgFmtDur(60000)).toBe("1m");
    expect(dgFmtDur(90000)).toBe("1m 30s");
  });
});

describe("dgVerdictUx", () => {
  it("maps known verdicts to a pill + meaning", () => {
    expect(dgVerdictUx("pass").pill).toBe("✓ Verified");
    expect(dgVerdictUx("hold").pill).toContain("Held");
    expect(dgVerdictUx("reject").pill).toContain("Needs review");
    expect(dgVerdictUx("pr-failed").pill).toContain("PR failed");
    expect(dgVerdictUx("advisory").mean).toContain("advisory");
  });
  it("falls back to a generic Done for unknown verdicts", () => {
    expect(dgVerdictUx("banana")).toEqual({
      pill: "Done",
      mean: "The run finished — open the report to read the findings.",
    });
  });
});

describe("dgReviewData", () => {
  it("returns null when nothing to show", () => {
    expect(dgReviewData(null)).toBeNull();
    expect(dgReviewData({})).toBeNull();
    expect(dgReviewData({ review_decision: "", review_findings: [] })).toBeNull();
  });
  it("normalizes the decision and coerces list/crap shapes", () => {
    const rd = dgReviewData({
      review_decision: "APPROVE",
      review_findings: [{ severity: "high" }],
      crap_delta: { gate_would_block: true },
      review_calibrated: false,
    });
    expect(rd.dec).toBe("approve");
    expect(rd.findings).toHaveLength(1);
    expect(rd.crap).toEqual({ gate_would_block: true });
    expect(rd.calibrated).toBe(false);
  });
  it("renders when only calibration is known (not null)", () => {
    expect(dgReviewData({ review_calibrated: true })).not.toBeNull();
  });
  it("ignores a non-object crap_delta", () => {
    expect(dgReviewData({ review_decision: "approve", crap_delta: "nope" }).crap).toBeNull();
  });
});

describe("dgPush", () => {
  it("appends {kind,text} entries", () => {
    const w = { lines: [] };
    dgPush(w, "text", "hi");
    expect(w.lines).toEqual([{ kind: "text", text: "hi" }]);
  });
  it("caps the feed at DG_MAX_LINES (drops oldest)", () => {
    const w = { lines: [] };
    for (let i = 0; i < DG_MAX_LINES + 25; i++) dgPush(w, "text", "l" + i);
    expect(w.lines).toHaveLength(DG_MAX_LINES);
    expect(w.lines[0].text).toBe("l25"); // first 25 dropped
    expect(w.lines[w.lines.length - 1].text).toBe("l" + (DG_MAX_LINES + 24));
  });
});

describe("dgIngestLine", () => {
  const fresh = () => ({ status: "running", lines: [] });

  it("ignores blank lines", () => {
    const w = fresh();
    dgIngestLine(w, "   ");
    expect(w.lines).toHaveLength(0);
  });

  it("routes [stderr] lines to an error entry", () => {
    const w = fresh();
    dgIngestLine(w, "[stderr] Service temporarily unavailable");
    expect(w.lines[0].kind).toBe("error");
    expect(w.lines[0].text).toContain("Service temporarily unavailable");
  });

  it("shows the raw line on JSON parse failure", () => {
    const w = fresh();
    dgIngestLine(w, "not json at all");
    expect(w.lines[0].kind).toBe("raw");
    expect(w.lines[0].text).toBe("not json at all");
  });

  it("expands assistant content blocks (text + tool_use + tool_result)", () => {
    const w = fresh();
    dgIngestLine(w, JSON.stringify({
      type: "assistant",
      message: { content: [
        { type: "text", text: "  working on it  " },
        { type: "tool_use", name: "Edit", input: { path: "a.js" } },
        { type: "tool_result" },
      ] },
    }));
    expect(w.lines.map((l) => l.kind)).toEqual(["text", "tool", "text"]);
    expect(w.lines[0].text).toBe("working on it");
    expect(w.lines[1].text).toContain("🔧 Edit");
    expect(w.lines[2].text).toBe("↳ tool result");
  });

  it("marks the worker done + pushes a result on a result line", () => {
    const w = fresh();
    dgIngestLine(w, JSON.stringify({ type: "result", is_error: false }));
    expect(w.status).toBe("done");
    expect(w.lines[0]).toEqual({ kind: "result", text: "✓ done" });
  });

  it("does NOT overwrite a retired status on result", () => {
    const w = { status: "retired", lines: [] };
    dgIngestLine(w, JSON.stringify({ type: "result", is_error: true }));
    expect(w.status).toBe("retired");
    expect(w.lines[0].text).toBe("✗ ended with error");
  });

  it("swallows system lines (signal-dense feed)", () => {
    const w = fresh();
    dgIngestLine(w, JSON.stringify({ type: "system", subtype: "init" }));
    expect(w.lines).toHaveLength(0);
  });

  it("caps a huge text block at ingest", () => {
    const w = fresh();
    const big = "x".repeat(5000);
    dgIngestLine(w, JSON.stringify({ type: "assistant", message: { content: [{ type: "text", text: big }] } }));
    expect(w.lines[0].text.length).toBe(400);
  });
});
