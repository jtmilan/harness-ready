// Agent Teams — EXTERNAL-spawn clamp: PURE cap resolution + pane expansion for the
// "external-spawn" Tauri event (LLM brain → UDS socket → app).
//
// The DOM listener lives in main.js (the `external-spawn` tauriEvent.listen block); this
// is the pure half, unit-testable in node with no DOM — the same pure-module split as
// bench-core.js / rail-core.js / board-core.js.
//
// The cap arrives CONFIG-RESOLVED on the payload (`p.cap`, additive): Rust resolves the
// operator's `external_spawn_max_panes` (mcp-config.json) via external_spawn_cap. An old
// backend emits no `cap` → default 8. The FE still enforces it (the brain is a
// prompt-injection-class caller) but never trusts a payload past the hard ceiling 16.

export const EXTERNAL_SPAWN_DEFAULT_CAP = 8;   // old backends emit no `cap`
export const EXTERNAL_SPAWN_HARD_CEILING = 16; // = backend clamp top; never trust payload beyond it

// The effective pane cap for one external-spawn event: the payload's config-resolved
// `cap` clamped to 1..=16 (mirrors the Rust external_spawn_cap clamp); a missing /
// non-numeric / zero cap falls back to the pre-`cap` default 8.
export function externalSpawnCap(payload) {
  const raw = Math.floor(Number(payload && payload.cap) || EXTERNAL_SPAWN_DEFAULT_CAP);
  return Math.min(EXTERNAL_SPAWN_HARD_CEILING, Math.max(1, raw));
}

// Expand per-pane spec groups → the flat parallel arrays createWorkspace consumes,
// clamped to `cap` (first-groups-win). `requested`/`truncated` let the confirm dialog
// tell the human the truth when the brain asked for more than this install allows.
export function expandExternalPanes(groups, cap) {
  const harnesses = [], roles = [], models = [];
  let requested = 0;
  for (const g of groups) {
    const n = Math.max(1, Number(g.count) || 1);
    requested += n;
    for (let i = 0; i < n && harnesses.length < cap; i++) {
      harnesses.push(g.harness || "claude");
      roles.push(g.role || "none");
      models.push(g.model || undefined);
    }
  }
  return { harnesses, roles, models, requested, truncated: requested > harnesses.length };
}
