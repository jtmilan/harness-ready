// Agent Teams — workspace presets (Phase 0: store only, no UI yet).
//
// A Preset is a reusable workspace template: a named, per-pane list of harnesses
// ("4 × Claude", "2 Claude + 2 Cursor") plus optional folder/seed-prompt. It is the
// existing `workspaces[wsId]` shape minus runtime fields (paneIds/sessionIds/counter/
// dormant). The wizard (Phase 1) reads/writes presets here and projects the chosen
// one onto createWorkspace().
//
// Schema:
//   { id, name, harnesses[], count(=harnesses.length, DERIVED), folder?, seedPrompt?,
//     color?, builtin? }
//
// INVARIANT: count === harnesses.length. It is structural, not checked — count is
// always DERIVED from harnesses, never trusted from input. `harnesses[]` is the SSOT.
//
// Persistence: USER presets live in localStorage `at_presets` ({v,presets[]}); BUILT-IN
// presets live in code (below) so first-run seeding and non-deletability are free and an
// app upgrade never leaves stale built-ins behind. listPresets() = builtins + user.
//
// Pure module: no imports from main.js / Tauri, so it is unit-testable in isolation
// (see presets.test.js). main.js imports FROM here in Phase 1.

export const STORAGE_KEY = "at_presets";
export const ACTIVE_KEY = "at_active_preset";
export const ENVELOPE_VERSION = 1;

// Harness ids the app can currently spawn. Phase 2 replaces this with the live harness
// catalog (a harness_catalog() Tauri command). Until then, keep in sync with
// parse_harness() (app/src-tauri/src/lib.rs:235) and the #f-harness <select>
// (app/src/index.html:108-112).
export const KNOWN_HARNESSES = ["claude", "codex", "commandcode", "cursor", "opencode", "cline", "grok", "bash"];
export function isKnownHarness(id) { return KNOWN_HARNESSES.includes(id); }

// Round-robin an ORDERED, multi-selected harness set across `count` panes: pane i takes
// selected[i % selected.length]. This is the "alternate cursor + claude" expansion the
// add-agent modal's harness chips feed to createWorkspace/addAgentsToWorkspace. Unknown
// ids are dropped; an empty/invalid selection falls back to all-"claude" so a spawn is
// never harness-less. Pure + total → unit-tested without the DOM.
export function expandHarnesses(selected, count) {
  const base = (Array.isArray(selected) ? selected.filter(isKnownHarness) : []);
  const set = base.length ? base : ["claude"];
  const n = Math.max(0, count | 0);
  return Array.from({ length: n }, (_, i) => set[i % set.length]);
}

// Role ids the wizard can assign per pane (17-01). "none" = homogeneous (no role,
// today's default). The rest map 1:1 to core/roles `AgentRole` — keep in sync with
// that enum + the app's `parse::<AgentRole>` (app/src-tauri) AND the external-spawn
// `external_spawn_role_allowed` (core/mcp). Bash panes ignore roles. The wizard
// stores "none" for the no-role slot; createWorkspace maps "none" → undefined at spawn.
export const KNOWN_ROLES = ["none", "coordinator", "builder", "scout", "reviewer", "tester", "performance", "security", "db-migration"];

// Shipped presets. builtin:true => read-only (duplicate to edit), never persisted to
// localStorage, never deletable. ids are stable + readable ("builtin-*").
export const BUILTIN_PRESETS = [
  { id: "builtin-solo-claude", name: "Solo Claude", harnesses: ["claude"], builtin: true },
  { id: "builtin-4-claude", name: "4 × Claude", harnesses: ["claude", "claude", "claude", "claude"], builtin: true },
  { id: "builtin-4-cursor", name: "4 × Cursor", harnesses: ["cursor", "cursor", "cursor", "cursor"], builtin: true },
  { id: "builtin-claude-cursor", name: "2 Claude + 2 Cursor", harnesses: ["claude", "claude", "cursor", "cursor"], builtin: true },
  { id: "builtin-4-codex", name: "4 × Codex", harnesses: ["codex", "codex", "codex", "codex"], builtin: true },
  { id: "builtin-claude-codex-cursor", name: "Claude + Codex + Cursor", harnesses: ["claude", "codex", "cursor"], builtin: true },
  { id: "builtin-4-commandcode", name: "4 × CommandCode", harnesses: ["commandcode", "commandcode", "commandcode", "commandcode"], builtin: true },
  { id: "builtin-claude-commandcode-cursor", name: "Claude + CommandCode + Cursor", harnesses: ["claude", "commandcode", "cursor"], builtin: true },
  { id: "builtin-4-cline", name: "4 × Cline", harnesses: ["cline", "cline", "cline", "cline"], builtin: true },
];

// ---- internals -------------------------------------------------------------

function genId() {
  try {
    if (typeof crypto !== "undefined" && crypto.randomUUID) return "preset-" + crypto.randomUUID();
  } catch (_) { /* fall through */ }
  return "preset-" + Date.now().toString(36) + "-" + Math.random().toString(36).slice(2, 8);
}

// localStorage access mirrors main.js's defensive try/catch-and-default style. Reads
// globalThis.localStorage each call (so a test can swap it per case); a no-op when
// storage is absent (built-ins still work; user presets just don't persist).
function lsGet(key) {
  try { return globalThis.localStorage ? globalThis.localStorage.getItem(key) : null; } catch (_) { return null; }
}
function lsSet(key, val) {
  try { if (globalThis.localStorage) globalThis.localStorage.setItem(key, val); } catch (_) { /* ignore */ }
}
function lsRemove(key) {
  try { if (globalThis.localStorage) globalThis.localStorage.removeItem(key); } catch (_) { /* ignore */ }
}

// "2 × claude + 2 × cursor" style label, used when a preset has no explicit name.
function harnessSummary(harnesses) {
  const counts = {};
  for (const h of harnesses) counts[h] = (counts[h] || 0) + 1;
  return Object.entries(counts).map(([h, n]) => `${n} × ${h}`).join(" + ");
}

// Persist only schema fields (drop derived/runtime flags like `unspawnable`).
function stripForStorage(p) {
  const out = { id: p.id, name: p.name, harnesses: p.harnesses.slice() };
  if (p.folder) out.folder = p.folder;
  if (p.seedPrompt) out.seedPrompt = p.seedPrompt;
  if (p.color) out.color = p.color;
  return out;
}

function loadUserPresets() {
  let env;
  try { env = JSON.parse(lsGet(STORAGE_KEY) || "null"); } catch (_) { env = null; }
  const arr = env && Array.isArray(env.presets) ? env.presets : [];
  // user presets are never built-in, whatever the stored bytes claim
  return arr.map((p) => normalizePreset({ ...p, builtin: false })).filter(Boolean);
}

function saveUserPresets(list) {
  lsSet(STORAGE_KEY, JSON.stringify({ v: ENVELOPE_VERSION, presets: list.map(stripForStorage) }));
}

// ---- normalization (lenient, for reads) ------------------------------------

// Coerce an arbitrary object into a valid Preset. Lenient: used for reading stored +
// built-in data, so unknown harnesses are KEPT (flagged `unspawnable`) for forward-
// compat (a preset survives a harness being removed). Enforces the invariant by
// DERIVING count. Returns null only when there is no usable harness to salvage.
export function normalizePreset(p) {
  if (!p || typeof p !== "object") return null;
  const harnesses = Array.isArray(p.harnesses) ? p.harnesses.filter((h) => typeof h === "string" && h) : [];
  if (harnesses.length === 0) return null;
  const out = {
    id: typeof p.id === "string" && p.id ? p.id : genId(),
    name: typeof p.name === "string" && p.name.trim() ? p.name.trim() : harnessSummary(harnesses),
    harnesses,
    count: harnesses.length, // DERIVED — the invariant is structural, never trusted from input
    builtin: p.builtin === true,
    unspawnable: harnesses.some((h) => !isKnownHarness(h)), // forward-compat: a removed harness
  };
  if (typeof p.folder === "string" && p.folder) out.folder = p.folder;
  if (typeof p.seedPrompt === "string" && p.seedPrompt) out.seedPrompt = p.seedPrompt;
  if (typeof p.color === "string" && p.color) out.color = p.color;
  return out;
}

// ---- CRUD ------------------------------------------------------------------

// All presets, built-ins first. Each is normalized (count derived, flags computed).
export function listPresets() {
  return [...BUILTIN_PRESETS.map(normalizePreset), ...loadUserPresets()];
}

export function getPreset(id) {
  return listPresets().find((p) => p.id === id) || null;
}

// Create or update a USER preset. STRICT: at least one harness, and every harness must
// be known (a stale id can be read leniently but not freshly saved). Built-ins are
// immutable — saving with a "builtin-*" id throws (duplicate to fork one). Returns the
// normalized, persisted preset.
export function savePreset(p) {
  if (!p || typeof p !== "object") throw new Error("preset must be an object");
  const harnesses = Array.isArray(p.harnesses) ? p.harnesses.filter((h) => typeof h === "string" && h) : [];
  if (harnesses.length === 0) throw new Error("preset needs at least one harness");
  const unknown = harnesses.filter((h) => !isKnownHarness(h));
  if (unknown.length) throw new Error("unknown harness: " + unknown.join(", "));
  if (typeof p.id === "string" && p.id.startsWith("builtin-")) {
    throw new Error("built-in presets are read-only; duplicate to edit");
  }
  const norm = normalizePreset({ ...p, harnesses, builtin: false });
  if (!norm) throw new Error("invalid preset");
  const users = loadUserPresets();
  const i = users.findIndex((u) => u.id === norm.id);
  if (i >= 0) users[i] = norm; else users.push(norm);
  saveUserPresets(users);
  return norm;
}

// Delete a USER preset. Built-ins cannot be deleted (throws). Returns true if something
// was removed. Clears the active pointer if it referenced the deleted preset.
export function deletePreset(id) {
  if (typeof id === "string" && id.startsWith("builtin-")) {
    throw new Error("built-in presets cannot be deleted");
  }
  const users = loadUserPresets();
  const next = users.filter((u) => u.id !== id);
  const removed = next.length !== users.length;
  if (removed) saveUserPresets(next);
  if (getActivePresetId() === id) setActivePresetId(null);
  return removed;
}

// ---- active-preset pointer (which preset the wizard last applied) -----------

export function getActivePresetId() { return lsGet(ACTIVE_KEY) || null; }
export function setActivePresetId(id) {
  if (id == null) { lsRemove(ACTIVE_KEY); return; }
  lsSet(ACTIVE_KEY, String(id));
}
