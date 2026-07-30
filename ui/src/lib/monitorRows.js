// F-OBS-1 (R-OBS) + A-VIS-1 (R5): pure, DOM-free view-model + lane semantics for the
// Monitoring health table and the TopBar fleet counts. Keeping this module free of JSX lets
// the honesty contract be unit-tested without a DOM/RTL dependency (R10: no new deps).
//
// Monitoring's hero is a per-pane real-signal table sourced from bridge.subscribe (the agents
// array). Columns split into EXISTS signals (id/kind/role/status/attention/branch/worktree —
// on the contract today) and NEEDS-BACKEND metrics (cpu/mem/git-dirty/last-tool-failure/
// human-gate queue-depth — NOT on the contract until R-OBS sampling + outcome events land,
// see docs/proposals/SYNTHESIS.md §2/§3).
//
// Honesty rule (R4 / fail-closed): a metric the backend does not yet provide — or an
// untrustworthy value (null/undefined/NaN/±Infinity) — renders as an explicit no-data cell,
// NEVER zero, NEVER interpolated. A real zero from a live feed stays a value.

// Status → semantic lane. needs_input + blocked both need the operator (amber/need); error is
// a hard failure (red/danger); working is healthy (green/success); starting is in-flight
// (cyan/info); idle carries no state signal (neutral/muted — never a sacred lane color).
export const STATUS_LANE = {
  working: "success",
  needs_input: "need",
  blocked: "need",
  error: "danger",
  starting: "info",
  idle: "muted",
};

// Lane → tailwind classes, structured so consumers never string-split a combined class
// (review fix: the old `.split(" ")[0]` hack was fragile). `text`/`border`/`dot` are the three
// cues; state is always text + dot shape, never hue alone (R8).
export const LANE = {
  need: { text: "text-need", border: "border-need", dot: "bg-need" },
  success: { text: "text-success", border: "border-success", dot: "bg-success" },
  danger: { text: "text-danger", border: "border-danger", dot: "bg-danger" },
  info: { text: "text-info", border: "border-info", dot: "bg-info" },
  muted: { text: "text-muted-foreground", border: "border-border", dot: "bg-muted-foreground" },
};
export const LANES = Object.keys(LANE);

// A metric that is absent OR non-finite is no-data. An explicit zero from a real feed is a
// legitimate value and must NOT collapse to no-data.
export function metricCell(v) {
  if (v === null || v === undefined) return { kind: "nodata" };
  if (typeof v === "number" && !Number.isFinite(v)) return { kind: "nodata" };
  return { kind: "value", value: v };
}

function fmtAttention(attention) {
  if (!attention) return null;
  const since =
    typeof attention.since === "number" && Number.isFinite(attention.since) ? attention.since : null;
  return {
    reason: typeof attention.reason === "string" && attention.reason ? attention.reason : "needs input",
    waitedSec: since === null ? null : Math.max(0, Math.floor((Date.now() - since) / 1000)),
  };
}

// Compact elapsed-time formatting for the attention column. Pure + unit-tested.
export function fmtWaited(sec) {
  if (sec === null || sec === undefined) return "";
  if (sec < 60) return `${sec}s`;
  const m = Math.floor(sec / 60);
  const s = sec % 60;
  return s ? `${m}m${s}s` : `${m}m`;
}

export function buildMonitorRows(agents) {
  return (Array.isArray(agents) ? agents : []).map((a) => ({
    id: a.id,
    name: a.name || a.id,
    kind: a.kind ?? null,
    role: a.role ?? null,
    status: a.status ?? null,
    lane: STATUS_LANE[a.status] || "muted",
    attention: fmtAttention(a.attention),
    branch: a.branch ?? null,
    worktree: a.worktree ?? null,
    metrics: {
      cpu: metricCell(a.cpu ?? null),
      mem: metricCell(a.mem ?? null),
      gitDirty: metricCell(a.gitDirty ?? null),
      lastToolFailure: metricCell(a.lastToolFailure ?? null),
      queueDepth: metricCell(a.queueDepth ?? null),
    },
  }));
}

// NEEDS-BACKEND metric columns, in display order. `needsBackend:true` is the honest tag the
// header renders as a "pending R-OBS" affordance so a reader never mistakes an empty column
// for a finished differentiator.
export const METRIC_COLS = [
  { key: "cpu", label: "CPU", needsBackend: true },
  { key: "mem", label: "MEM", needsBackend: true },
  { key: "gitDirty", label: "GIT", needsBackend: true },
  { key: "lastToolFailure", label: "LAST FAIL", needsBackend: true },
  { key: "queueDepth", label: "GATE Q", needsBackend: true },
];

// Live, subscribe-derived fleet counts (single source of truth shared by TopBar + Monitoring,
// C1): live = every pane present; needsYou = panes the operator must answer (needs_input +
// blocked); working = the old "active" definition. Fixes the ACTIVE-AGENTS undercount without
// inventing data. Unknown / missing statuses are counted in `live` but not in any lane bucket.
export function fleetCounts(agents) {
  const c = { working: 0, needs_input: 0, blocked: 0, error: 0, starting: 0, idle: 0 };
  const list = Array.isArray(agents) ? agents : [];
  for (const a of list) {
    if (a && a.status && c[a.status] !== undefined) c[a.status] += 1;
  }
  return { ...c, live: list.length, needsYou: c.needs_input + c.blocked };
}
