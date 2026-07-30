// F-TPL-2 (R-TEMPLATES / R6): versioned JSON import/export + validation for team templates.
// The on-disk store stays localAgentTemplateStore (localStorage) — this module only moves
// templates in/out as a portable, schema-pinned bundle (anti-lock-in / portability). No new
// deps. Pure helpers are unit-tested; downloadJSON is the one DOM-touching helper.

import { coerceTemplateAgents } from "@/components/command/templates/templateAgents";

export const TEMPLATE_SCHEMA_VERSION = 1;
const MAX_IMPORT = 500; // localStorage is finite; refuse a runaway bundle up front.

const isObj = (v) => v !== null && typeof v === "object" && !Array.isArray(v);

/**
 * Validate + normalize ONE template object. Ids/timestamps are intentionally NOT carried —
 * create() regenerates them, so import = append-without-clobber (safe across machines).
 * @param {unknown} raw
 * @returns {{ ok: true, template: { name: string, description: string, agents: object[] } } | { ok: false, error: string }}
 */
export function validateOneTemplate(raw) {
  if (!isObj(raw)) return { ok: false, error: "not an object" };
  // Cap lengths: import is the untrusted-input path; a pathological 50 MB description would
  // blow the localStorage quota and evict other app state. Trim → emptiness check → slice.
  const name = (typeof raw.name === "string" ? raw.name.trim() : "").slice(0, 200);
  if (!name) return { ok: false, error: "missing/empty name" };
  if (raw.agents !== undefined && !Array.isArray(raw.agents)) {
    return { ok: false, error: "agents must be an array" };
  }
  const description = (typeof raw.description === "string" ? raw.description : "").slice(0, 2000);
  return {
    ok: true,
    template: { name, description, agents: coerceTemplateAgents(raw.agents) },
  };
}

/**
 * Parse an import payload. Accepts a bare array, a `{ schema, templates: [] }` bundle, or a
 * single template object. Valid templates are returned; malformed ones are reported per-index
 * in `skipped` (honest partial import — the file is the operator's own data, not an untrusted
 * input, so we import the good and name the bad rather than fail the whole file).
 * @param {string} text
 * @returns {{ ok: true, templates: object[], skipped: { index: number, error: string }[] } | { ok: false, errors: string[] }}
 */
export function parseImportPayload(text) {
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch (e) {
    return { ok: false, errors: [`invalid JSON: ${e.message}`] };
  }

  let list;
  if (Array.isArray(parsed)) {
    list = parsed;
  } else if (isObj(parsed) && Array.isArray(parsed.templates)) {
    if (typeof parsed.schema === "number" && parsed.schema > TEMPLATE_SCHEMA_VERSION) {
      return { ok: false, errors: [`schema ${parsed.schema} is newer than supported ${TEMPLATE_SCHEMA_VERSION}`] };
    }
    list = parsed.templates;
  } else if (isObj(parsed) && typeof parsed.name === "string") {
    list = [parsed];
  } else {
    return { ok: false, errors: ["expected an array, a { templates: [] } bundle, or a single template object"] };
  }

  if (list.length > MAX_IMPORT) {
    return { ok: false, errors: [`too many templates in one import (${list.length} > ${MAX_IMPORT})`] };
  }

  const templates = [];
  const skipped = [];
  list.forEach((raw, index) => {
    const r = validateOneTemplate(raw);
    if (r.ok) templates.push(r.template);
    else skipped.push({ index, error: r.error });
  });
  return { ok: true, templates, skipped };
}

/** Build a portable export bundle (ids/timestamps stripped; agents coerced). */
export function buildExportBundle(templates) {
  const safe = Array.isArray(templates) ? templates : [];
  return {
    schema: TEMPLATE_SCHEMA_VERSION,
    source: "harness-ready",
    exported_at: new Date().toISOString(),
    templates: safe.map((t) => ({
      name: typeof t?.name === "string" ? t.name : "",
      description: typeof t?.description === "string" ? t.description : "",
      agents: coerceTemplateAgents(t?.agents),
    })),
  };
}

/** Trigger a browser download of `text` as a .json file. No-op outside a DOM (ssr/tests). */
export function downloadJSON(filename, text) {
  if (typeof document === "undefined" || typeof Blob === "undefined") return;
  const blob = new Blob([text], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.rel = "noopener";
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}
