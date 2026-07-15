// Template agent rows must carry a real harness `kind` (HARNESS_WIRE keys in
// tauriAgentBridge.js). The closed set is the same as AGENT_KINDS / KIND_IDS —
// TemplateBuilder only offers those, but older localStorage rows or hand-edits
// can still hold a persona string as `kind` (e.g. "architect"). After K4, that
// refuses at spawn; coerce here so launch/save never emit an unmapped kind.

import { KIND_IDS } from "@/lib/agentTypes";

const KIND_SET = new Set(KIND_IDS);

// Schema / builder default — the harness a role-based team template expects when
// the operator only named a persona and never picked a CLI.
export const DEFAULT_TEMPLATE_KIND = "claude-code";

/**
 * @param {unknown} agents
 * @returns {{ role: string, kind: string, priority?: string, autonomy?: string }[]}
 */
export function coerceTemplateAgents(agents) {
  if (!Array.isArray(agents)) return [];
  return agents.map((raw) => {
    const a = raw && typeof raw === "object" ? raw : {};
    let kind = typeof a.kind === "string" ? a.kind.trim() : "";
    let role = typeof a.role === "string" ? a.role : "";
    if (!kind) {
      kind = DEFAULT_TEMPLATE_KIND;
    } else if (!KIND_SET.has(kind)) {
      // Persona mistaken for harness: keep the label as role when role is empty.
      if (!role.trim()) role = kind;
      kind = DEFAULT_TEMPLATE_KIND;
    }
    return { ...a, kind, role };
  });
}
