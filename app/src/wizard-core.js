// Agent Teams — layout wizard core (Phase 1: pure model + reducers, no UI).
//
// The 3-step create wizard (Start → Layout → Agents) is split into a PURE state
// model (here) and a DOM controller (wizard.js). This module owns the wizard's
// shape and its transitions; it has NO DOM, NO globals, NO Tauri — every reducer
// takes a state and returns a NEW state, so wizard.js can render off the result
// and wizard-core.test.js can exercise the whole flow in node.
//
// State (plain object; treat as immutable — reducers spread a fresh copy):
//   { step:1|2|3, name:string, folder:string, count:number,
//     harnesses:string[]  (length === count, INVARIANT),
//     roles:string[]      (length === count, INVARIANT; "none" = roleless),
//     models:string[]     (length === count, INVARIANT; "" = account default),
//     seedPrompt:string, activePresetId:string|null }
//
// INVARIANT: harnesses.length === count === roles.length. count is the SSOT for the
// pane count; harnesses + roles are fit (padded/truncated) to it after every
// count/preset change via fit()/fitRoles(). (presets.js inverts this for harnesses —
// there harnesses is the SSOT and count is derived — because a stored preset is data,
// whereas the wizard count is a knob the user turns directly. Roles are NOT persisted
// in a preset today — they reset to "none" on preset apply.)
//
// Pure module: imports ONLY ./presets.js (for KNOWN_HARNESSES, the harness-id
// SSOT). Phase 2 swaps that for a live harness catalog.

import { KNOWN_HARNESSES, KNOWN_ROLES } from "./presets.js";

export const DEFAULT_HARNESS = "claude";
export const DEFAULT_ROLE = "none"; // 17-01: a roleless (homogeneous) pane
export const TILE_COUNTS = [1, 2, 4, 6, 9, 10, 12]; // ruling A: keep 9

// basename of a path, slash-delimited: drop empties so a trailing "/" is ignored.
function basename(p) { return String(p || "").split("/").filter(Boolean).pop() || ""; }

// Fit a harness list to exactly `count` slots: truncate when too long, else pad
// with the last existing harness (so growing "4 × cursor" stays cursor) falling
// back to DEFAULT_HARNESS for an empty list. Returns a NEW array (never aliases
// the input), so callers stay pure. count is assumed already clamped to >= 1.
function fit(harnesses, count) {
  const out = harnesses.slice(0, count);
  const pad = out.length ? out[out.length - 1] : DEFAULT_HARNESS;
  while (out.length < count) out.push(pad);
  return out;
}

// Fit a ROLE list to exactly `count` slots: truncate when too long, else pad with
// DEFAULT_ROLE ("none"). Unlike harness `fit` (which propagates the last value so a
// "4 × cursor" stays cursor), a GROWN role list pads with "none" — a brand-new pane
// is roleless until the user explicitly assigns one. Returns a NEW array.
function fitRoles(roles, count) {
  const out = roles.slice(0, count);
  while (out.length < count) out.push(DEFAULT_ROLE);
  return out;
}

// Fit a MODEL list to exactly `count` slots (model-at-spawn): pad with "" — a new
// pane runs on its harness's account default until the user types a model.
function fitModels(models, count) {
  const out = (models || []).slice(0, count);
  while (out.length < count) out.push("");
  return out;
}

export function createInitialState(opts = {}) {
  const count = Math.max(1, opts.count | 0); // |0 coerces; Array(NaN|-1) would throw
  // Validate the seed harness against the SSOT so a stale id can't seed the grid;
  // unknown ids fall back to DEFAULT_HARNESS (defensive, like presets.js reads).
  const reqHarness = opts.defaultHarness || DEFAULT_HARNESS;
  const seed = KNOWN_HARNESSES.includes(reqHarness) ? reqHarness : DEFAULT_HARNESS;
  // Scheduler cap snapshot (05-02, D33) for the layout-step overflow hint. cap=null
  // ⇒ unknown (browser harness / pre-first-poll) ⇒ no hint. working = live `working`
  // count at open time (a static open-time snapshot is enough to warn N panes won't
  // all fit — exact parking is the Scheduler's job).
  const cap = Number.isFinite(opts.cap) && opts.cap >= 1 ? (opts.cap | 0) : null;
  const working = Number.isFinite(opts.working) && opts.working >= 0 ? (opts.working | 0) : 0;
  return {
    step: 1,
    name: opts.name || "",
    folder: opts.folder || "",
    count,
    harnesses: Array(count).fill(seed),
    roles: Array(count).fill(DEFAULT_ROLE), // 17-01: per-pane role (INVARIANT: length === count)
    models: Array(count).fill(""), // model-at-spawn: per-pane model ("" = account default)
    seedPrompt: "",
    activePresetId: null,
    cap,
    working,
  };
}

// How many of the chosen panes will overflow the admission cap and QUEUE (not error —
// {queued,position} is success, D33). free = cap − working (already-running agents
// count against the cap). 0 when cap is unknown.
export function overflowCount(state) {
  if (state.cap == null) return 0;
  const free = Math.max(0, state.cap - state.working);
  return Math.max(0, state.count - free);
}

// ---- step navigation (none of these validate — canAdvance() is the gate) -----

export function setStep(state, step) {
  return { ...state, step: Math.min(3, Math.max(1, step | 0)) };
}
export function goNext(state) { return setStep(state, state.step + 1); }
export function goBack(state) { return setStep(state, state.step - 1); }

// ---- plain field setters (store raw; do NOT desync the active preset) --------

export function setName(state, name) { return { ...state, name: String(name) }; }
export function setFolder(state, folder) { return { ...state, folder: String(folder) }; }
export function setSeedPrompt(state, p) { return { ...state, seedPrompt: String(p) }; }

// ---- layout (count + per-pane harness) — these DO desync the active preset ---

export function setCount(state, n) {
  const count = Math.max(1, n | 0);
  return {
    ...state,
    count,
    harnesses: fit(state.harnesses, count),
    roles: fitRoles(state.roles || [], count),
    models: fitModels(state.models, count),
    activePresetId: null,
  };
}

export function setPaneHarness(state, i, harness) {
  // Out of range = no-op: return the SAME state (don't desync, don't reallocate).
  if (!(i >= 0 && i < state.count)) return state;
  const harnesses = state.harnesses.slice(); // clone before edit — spread is shallow
  harnesses[i] = harness;
  return { ...state, harnesses, activePresetId: null };
}

// 17-01: set one pane's ROLE (mirror setPaneHarness). Out of range = no-op. An
// unknown role id is stored as-is (the UI only offers KNOWN_ROLES); "none" = roleless.
// Does NOT desync the active preset — presets are harness-only today, so a role pick
// is orthogonal to the applied preset (the highlight stays).
export function setPaneRole(state, i, role) {
  if (!(i >= 0 && i < state.count)) return state;
  const roles = (state.roles || fitRoles([], state.count)).slice();
  roles[i] = role;
  return { ...state, roles };
}

// model-at-spawn: set one pane's MODEL (mirror setPaneRole). Out of range = no-op.
// Stored verbatim ("" = account default) — the harness CLI validates the id itself.
// Orthogonal to the applied preset, like roles.
export function setPaneModel(state, i, model) {
  if (!(i >= 0 && i < state.count)) return state;
  const models = fitModels(state.models, state.count);
  models[i] = String(model || "");
  return { ...state, models };
}

// ---- preset / no-AI projections ---------------------------------------------

// Apply a normalized Preset (from presets.js). count comes from the preset; the
// preset's harnesses are fit to it (a no-op for a normalized preset, where
// count === harnesses.length, but defends against a malformed one). Ruling D —
// "wizard wins, preset pre-fills": keep the user's folder/name/seedPrompt if set,
// else take the preset's. activePresetId tracks the applied preset.
export function applyPreset(state, preset) {
  const count = Math.max(1, preset.count | 0);
  return {
    ...state,
    count,
    harnesses: fit(preset.harnesses.slice(), count),
    // presets are harness-only today (roles not persisted) → a preset gives a fresh
    // layout, so reset roles to "none" for its count. The user assigns roles after.
    roles: Array(count).fill(DEFAULT_ROLE),
    models: Array(count).fill(""),
    folder: state.folder.trim() ? state.folder : (preset.folder || ""),
    name: state.name.trim() ? state.name : (preset.name || ""),
    seedPrompt: state.seedPrompt ? state.seedPrompt : (preset.seedPrompt || ""),
    activePresetId: preset.id,
  };
}

// "Open without AI" = every pane is a plain bash terminal (and roleless). Keeps the
// current count; clears the preset (the all-bash layout is not a saved preset).
export function openWithoutAI(state) {
  return {
    ...state,
    harnesses: Array(state.count).fill("bash"),
    roles: Array(state.count).fill(DEFAULT_ROLE),
    models: Array(state.count).fill(""),
    activePresetId: null,
  };
}

// ---- validation / projection -------------------------------------------------

// Whether the current step may advance. Step 1 needs a working folder; steps 2
// and 3 are always advanceable (layout/harness have sensible defaults).
export function canAdvance(state) {
  if (state.step === 1 && !state.folder.trim()) {
    return { ok: false, reason: "working folder is required" };
  }
  return { ok: true, reason: "" };
}

export function isLastStep(state) { return state.step === 3; }

// Project the finished wizard onto createWorkspace()'s argument shape. name falls
// back to the folder basename then a literal default; harness is pane 0 (kept so
// reopen/add-agent have a workspace default). NO color — main.js adds it.
export function toCreateArgs(state) {
  const repo = state.folder.trim();
  return {
    name: state.name.trim() || basename(repo) || "workspace",
    repo,
    count: state.count,
    harness: state.harnesses[0],
    harnesses: state.harnesses.slice(),
    // 17-01: per-pane roles (parallel to harnesses; "none" = roleless). createWorkspace
    // maps "none" → undefined at spawn.
    roles: (state.roles || fitRoles([], state.count)).slice(),
    // model-at-spawn: per-pane models ("" = account default; createWorkspace maps to undefined).
    models: fitModels(state.models, state.count),
    prompt: state.seedPrompt,
  };
}

// Near-square grid for the layout preview: cols = ceil(sqrt(n)), rows fill the
// rest. e.g. 1→1×1, 2→1×2, 4→2×2, 6→2×3, 9→3×3, 10→4×3, 12→4×3.
export function gridShape(count) {
  const cols = Math.ceil(Math.sqrt(count));
  return { rows: Math.ceil(count / cols), cols };
}
