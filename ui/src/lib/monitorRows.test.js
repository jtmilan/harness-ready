import { describe, it, expect } from "vitest";
import {
  STATUS_LANE,
  LANE,
  LANES,
  metricCell,
  fmtWaited,
  buildMonitorRows,
  METRIC_COLS,
  fleetCounts,
} from "@/lib/monitorRows";

describe("metricCell (R4 honesty — fail closed; never collapse a real zero to no-data)", () => {
  it("marks absent metrics as no-data", () => {
    expect(metricCell(null)).toEqual({ kind: "nodata" });
    expect(metricCell(undefined)).toEqual({ kind: "nodata" });
  });
  it("keeps a real zero as a value, not no-data", () => {
    expect(metricCell(0)).toEqual({ kind: "value", value: 0 });
  });
  it("passes real finite values through", () => {
    expect(metricCell(42)).toEqual({ kind: "value", value: 42 });
  });
  it("treats non-finite numbers (NaN/Infinity) as no-data, not a value", () => {
    expect(metricCell(NaN)).toEqual({ kind: "nodata" });
    expect(metricCell(Infinity)).toEqual({ kind: "nodata" });
    expect(metricCell(-Infinity)).toEqual({ kind: "nodata" });
  });
});

describe("LANE / LANES (R5/R8 — every status lane must have a full class triple)", () => {
  it("exposes text+border+dot for every lane", () => {
    for (const name of LANES) {
      expect(LANE[name]).toEqual(
        expect.objectContaining({ text: expect.any(String), border: expect.any(String), dot: expect.any(String) }),
      );
    }
  });
  it("every STATUS_LANE value resolves to a defined lane (catches a class-map typo)", () => {
    for (const v of Object.values(STATUS_LANE)) expect(LANE[v]).toBeDefined();
  });
});

describe("STATUS_LANE", () => {
  it("attention states use need/danger; idle is neutral not a sacred lane", () => {
    expect(STATUS_LANE.needs_input).toBe("need");
    expect(STATUS_LANE.blocked).toBe("need");
    expect(STATUS_LANE.error).toBe("danger");
    expect(STATUS_LANE.working).toBe("success");
    expect(STATUS_LANE.starting).toBe("info");
    expect(STATUS_LANE.idle).toBe("muted");
  });
});

describe("fmtWaited", () => {
  it("formats seconds, minutes+seconds, exact minutes, zero, and absent", () => {
    expect(fmtWaited(5)).toBe("5s");
    expect(fmtWaited(125)).toBe("2m5s");
    expect(fmtWaited(120)).toBe("2m");
    expect(fmtWaited(0)).toBe("0s");
    expect(fmtWaited(null)).toBe("");
    expect(fmtWaited(undefined)).toBe("");
  });
});

describe("buildMonitorRows", () => {
  const makeAgents = () => [
    { id: "ws-1-p0", name: "AMBER", kind: "claude", role: "coordinator", status: "needs_input",
      attention: { reason: "approve tool", since: Date.now() - 5000 }, branch: "feat/x", worktree: "~/wt/0" },
    { id: "ws-1-p1", kind: "opencode", status: "working", cpu: 0, mem: null }, // real 0 cpu, absent mem
  ];

  it("projects EXISTS fields verbatim from the agent shape", () => {
    const rows = buildMonitorRows(makeAgents());
    expect(rows[0].id).toBe("ws-1-p0");
    expect(rows[0].role).toBe("coordinator");
    expect(rows[0].lane).toBe("need");
    expect(rows[0].branch).toBe("feat/x");
  });

  it("derives a live waitedSec from attention.since (>= the elapsed floor)", () => {
    const rows = buildMonitorRows(makeAgents());
    expect(rows[0].attention.reason).toBe("approve tool");
    expect(rows[0].attention.waitedSec).toBeGreaterThanOrEqual(5);
    expect(rows[1].attention).toBeNull();
  });

  it("defaults a missing/empty reason to 'needs input' and a missing since to null wait", () => {
    const rows = buildMonitorRows([
      { id: "a", status: "needs_input", attention: { reason: "", since: Date.now() } },
      { id: "b", status: "needs_input", attention: { reason: "x" } },
    ]);
    expect(rows[0].attention.reason).toBe("needs input");
    expect(rows[1].attention.reason).toBe("x");
    expect(rows[1].attention.waitedSec).toBeNull();
  });

  it("guards a non-finite since to a null wait (no NaN propagation)", () => {
    const rows = buildMonitorRows([{ id: "a", status: "needs_input", attention: { reason: "r", since: NaN } }]);
    expect(rows[0].attention.waitedSec).toBeNull();
  });

  it("renders every NEEDS-BACKEND metric as no-data when the feed is absent", () => {
    const rows = buildMonitorRows(makeAgents());
    for (const col of METRIC_COLS) expect(rows[0].metrics[col.key].kind).toBe("nodata");
    expect(rows[1].metrics.cpu).toEqual({ kind: "value", value: 0 });
    expect(rows[1].metrics.mem).toEqual({ kind: "nodata" });
  });

  it("falls back name→id and unknown/absent status→muted lane", () => {
    const rows = buildMonitorRows([
      { id: "ws-1-p9", status: "idle" },
      { id: "ws-1-pX", status: "panicking" }, // unknown status string
    ]);
    expect(rows[0].name).toBe("ws-1-p9");
    expect(rows[0].lane).toBe("muted");
    expect(rows[0].role).toBeNull();
    expect(rows[1].lane).toBe("muted");
  });

  it("handles a missing/empty fleet without throwing", () => {
    expect(buildMonitorRows(undefined)).toEqual([]);
    expect(buildMonitorRows([])).toEqual([]);
  });
});

describe("METRIC_COLS", () => {
  it("every metric column is honestly tagged NEEDS-BACKEND", () => {
    expect(METRIC_COLS.length).toBe(5);
    for (const col of METRIC_COLS) expect(col.needsBackend).toBe(true);
  });
});

describe("fleetCounts (C1 — single source for live/needsYou/working)", () => {
  it("counts live = all panes and needsYou = needs_input + blocked", () => {
    const c = fleetCounts([
      { status: "working" }, { status: "working" }, { status: "needs_input" },
      { status: "blocked" }, { status: "idle" },
    ]);
    expect(c.live).toBe(5);
    expect(c.needsYou).toBe(2);
    expect(c.working).toBe(2);
    expect(c.error).toBe(0);
  });
  it("counts unknown / null / missing statuses in live but in no lane bucket", () => {
    const c = fleetCounts([{ status: "panicking" }, { status: null }, {}]);
    expect(c.live).toBe(3);
    expect(c.working).toBe(0);
    expect(c.needsYou).toBe(0);
  });
  it("is safe on empty/absent fleet", () => {
    expect(fleetCounts([]).live).toBe(0);
    expect(fleetCounts(undefined).needsYou).toBe(0);
  });
});
