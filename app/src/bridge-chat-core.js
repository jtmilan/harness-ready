// Pure event→timeline model for the docked Bridge chat pane. No DOM, no Tauri, no
// globals — main.js's phase machine emits events, chatReduce folds them into a
// renderable conversation (turns) + live pane snapshot. Same pattern as rail-core.js.
//
// Reducer is TOTAL (unknown event → same state) and IMMUTABLE (never mutates input).
// Two anti-flood rules: in-flight phase turns (planning/collecting) REPLACE a
// consecutive same-phase turn ("thinking…" semantics), and pane updates fold into
// ONE live status turn while it stays last in the timeline.

export const KNOWN_HARNESSES = ["claude", "codex", "cursor", "opencode", "commandcode", "cline"];

const TURN_CAP = 200;
// In-flight phases that render as a transient agent status line.
const INFLIGHT = { planning: "Planning…", collecting: "Collecting reports…" };

export function initialChat() {
  // seq: monotonic turn-id counter (stable keys for the renderer).
  return { turns: [], phase: "idle", paneStates: {}, prd: null, seq: 0 };
}

// Trim from the front to the cap, but never drop the MOST RECENT plan/prd card —
// those carry live actions (Run Flywheel sits on the prd card) and must survive
// chatty runs.
function capTurns(turns) {
  if (turns.length <= TURN_CAP) return turns;
  let lastPlan = -1, lastPrd = -1;
  for (let i = turns.length - 1; i >= 0 && (lastPlan < 0 || lastPrd < 0); i--) {
    if (lastPlan < 0 && turns[i].kind === "plan") lastPlan = i;
    if (lastPrd < 0 && turns[i].kind === "prd") lastPrd = i;
  }
  let drop = turns.length - TURN_CAP;
  const out = [];
  for (let i = 0; i < turns.length; i++) {
    if (drop > 0 && i !== lastPlan && i !== lastPrd) { drop--; continue; }
    out.push(turns[i]);
  }
  return out;
}

function push(state, turn) {
  const seq = state.seq + 1;
  return { ...state, seq, turns: capTurns([...state.turns, { ...turn, seq }]) };
}

// Replace the LAST turn in place: same timeline slot, same seq, new content.
function replaceLast(state, turn) {
  const last = state.turns[state.turns.length - 1];
  return { ...state, turns: [...state.turns.slice(0, -1), { ...turn, seq: last.seq }] };
}

// Typed sub-agent roles the omnigent Dispatch envelope may carry (core/roles SSOT).
export const KNOWN_ROLES = ["coder", "builder", "tester", "coordinator", "reviewer", "scout"];

// Normalize a plan task: keep id/task/wave, and CARRY role + owns when present (no-op
// when absent — never invent fields, never drop the back-compat shape).
function planTask(d) {
  const t = { id: d.id, task: d.task, wave: d.wave };
  if (d.role != null && d.role !== "") t.role = d.role;
  if (Array.isArray(d.owns) && d.owns.length) t.owns = d.owns;
  return t;
}

// Pure formatter for a per-task role label, e.g. "[reviewer]". Returns "" when no role
// (main.js calls this to render the badge; DOM stays out of the pure core).
export function formatRoleBadge(role) {
  const r = String(role || "").trim();
  return r ? `[${r}]` : "";
}

export function chatReduce(state, event) {
  const e = event || {};
  switch (e.type) {
    case "goal":
      return push(state, { kind: "user", text: e.text });

    case "phase": {
      const next = { ...state, phase: e.phase };
      if (!(e.phase in INFLIGHT)) return next; // terminal phases set state only
      const turn = { kind: "phase", phase: e.phase, text: e.label || INFLIGHT[e.phase] };
      const last = state.turns[state.turns.length - 1];
      if (last && last.kind === "phase" && last.phase === e.phase) return replaceLast(next, turn);
      return push(next, turn);
    }

    case "plan":
      // Preserve each task's typed role (omnigent Dispatch envelope) + owns hint when
      // present; back-compat: a task with neither still reduces to {id,task,wave}.
      return push(state, { kind: "plan", tasks: (e.tasks || []).map(planTask), twoWave: !!e.twoWave });

    case "pane": {
      const paneStates = { ...state.paneStates, [e.id]: { text: e.text, cls: e.cls || null } };
      const next = { ...state, paneStates };
      const turn = { kind: "status", panes: paneStates }; // self-contained snapshot
      const last = state.turns[state.turns.length - 1];
      if (last && last.kind === "status") return replaceLast(next, turn);
      return push(next, turn);
    }

    case "dispatched": {
      const n = e.n || 0;
      const bits = [`Dispatched to ${n} pane${n === 1 ? "" : "s"}`];
      if (e.dead) bits.push(`${e.dead} dead skipped`);
      if (e.heldVerify) bits.push("verify pane held");
      return push(state, {
        kind: "dispatched", n, dead: e.dead || 0, heldVerify: !!e.heldVerify, text: bits.join(" · "),
      });
    }

    case "prd": {
      const prd = { path: e.path, ...(e.verdictHint != null ? { verdictHint: e.verdictHint } : {}) };
      return push({ ...state, prd }, { kind: "prd", path: e.path, verdictHint: e.verdictHint ?? null });
    }

    case "info":
    case "error":
      return push(state, { kind: e.type, text: e.text });

    case "reset":
      return initialChat(); // fresh run — keep nothing, seq restarts

    default:
      return state; // total reducer: unknown events are no-ops
  }
}

// ---- input classification (salvaged from the bridge-chat prototype) ----------

const NUMWORDS = { one: 1, two: 2, three: 3, four: 4, five: 5, six: 6, seven: 7, eight: 8, nine: 9, ten: 10, a: 1, an: 1, another: 1 };
const numFor = (tok) => (tok ? NUMWORDS[tok] ?? (/^\d+$/.test(tok) ? +tok : null) : null);

// "spin up 2 codex and 1 claude" -> { count: 3, harnesses: ["codex","codex","claude"] }.
// Harness names validate against the caller-supplied list (no presets.js import);
// no KNOWN harness mentioned -> null (garbage and unknown-harness both fall here).
export function parseSpawn(text, knownHarnesses = KNOWN_HARNESSES) {
  const t = String(text || "").toLowerCase();
  const hits = []; // [pos, harness, n] — kept in text order
  for (const h of knownHarnesses) {
    const re = new RegExp(`(?:(\\w+)\\s+)?(?:more\\s+|additional\\s+|new\\s+|extra\\s+)?${h}\\b`, "g");
    let m;
    while ((m = re.exec(t))) hits.push([m.index, h, numFor(m[1]) ?? 1]);
  }
  if (!hits.length) return null;
  hits.sort((a, b) => a[0] - b[0]);
  const harnesses = [];
  for (const [, h, n] of hits) for (let i = 0; i < n; i++) harnesses.push(h);
  return { count: harnesses.length, harnesses };
}

const SPAWN_VERB = /\b(spin up|launch|spawn|boot|add)\b/;

// "spawn" only on an explicit spawn verb + a parseable KNOWN harness; everything
// else is a goal. Guards the {status:ok} false-positive: a goal CONTAINING json
// braces (or the word "status") is still a goal.
export function classifyInput(text, knownHarnesses = KNOWN_HARNESSES) {
  const t = String(text || "").toLowerCase();
  if (SPAWN_VERB.test(t) && parseSpawn(t, knownHarnesses)) return "spawn";
  return "goal";
}
