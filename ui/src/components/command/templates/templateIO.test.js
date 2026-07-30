import { describe, it, expect } from "vitest";
import {
  TEMPLATE_SCHEMA_VERSION,
  validateOneTemplate,
  parseImportPayload,
  buildExportBundle,
  downloadJSON,
} from "@/components/command/templates/templateIO";

describe("validateOneTemplate", () => {
  it("accepts a minimal template and coerces agents", () => {
    const r = validateOneTemplate({ name: "  reviewers  ", agents: [{ role: "reviewer" }] });
    expect(r.ok).toBe(true);
    expect(r.template.name).toBe("reviewers");
    expect(r.template.description).toBe("");
    expect(r.template.agents[0].kind).toBe("claude-code"); // default applied
  });
  it("rejects missing/empty name and non-array agents", () => {
    expect(validateOneTemplate({ agents: [] }).ok).toBe(false);
    expect(validateOneTemplate({ name: "" }).ok).toBe(false);
    expect(validateOneTemplate({ name: "x", agents: "nope" }).ok).toBe(false);
    expect(validateOneTemplate(null).ok).toBe(false);
  });
  it("applies the cline→pi migration on import", () => {
    const r = validateOneTemplate({ name: "t", agents: [{ role: "r", kind: "cline" }] });
    expect(r.template.agents[0].kind).toBe("pi");
  });
  it("drops unknown agent keys (schema pin; no bloat/injection vector)", () => {
    const r = validateOneTemplate({ name: "t", agents: [{ role: "r", kind: "claude-code", EVIL: "x".repeat(100) }] });
    expect(r.template.agents[0]).toEqual({ kind: "claude-code", role: "r", priority: undefined, autonomy: undefined });
  });
  it("caps oversized name/description (localStorage quota guard)", () => {
    const r = validateOneTemplate({ name: "n".repeat(500), description: "d".repeat(5000) });
    expect(r.template.name.length).toBe(200);
    expect(r.template.description.length).toBe(2000);
  });
});

describe("parseImportPayload", () => {
  it("accepts a bare array", () => {
    const r = parseImportPayload(JSON.stringify([{ name: "a" }, { name: "b" }]));
    expect(r.ok).toBe(true);
    expect(r.templates).toHaveLength(2);
    expect(r.skipped).toEqual([]);
  });
  it("accepts a versioned bundle and a single object", () => {
    const bundle = parseImportPayload(JSON.stringify({ schema: 1, templates: [{ name: "a" }] }));
    expect(bundle.ok).toBe(true);
    expect(bundle.templates).toHaveLength(1);
    const single = parseImportPayload(JSON.stringify({ name: "solo" }));
    expect(single.ok).toBe(true);
    expect(single.templates[0].name).toBe("solo");
  });
  it("rejects invalid JSON, unknown shape, and too-new schema", () => {
    expect(parseImportPayload("{not json").ok).toBe(false);
    expect(parseImportPayload(JSON.stringify({ foo: "bar" })).ok).toBe(false);
    const tooNew = parseImportPayload(JSON.stringify({ schema: TEMPLATE_SCHEMA_VERSION + 1, templates: [] }));
    expect(tooNew.ok).toBe(false);
  });
  it("imports the valid and reports the invalid per-index (honest partial import)", () => {
    const r = parseImportPayload(JSON.stringify([{ name: "good" }, { agents: [] }, { name: "ok" }]));
    expect(r.ok).toBe(true);
    expect(r.templates.map((t) => t.name)).toEqual(["good", "ok"]);
    expect(r.skipped).toEqual([{ index: 1, error: "missing/empty name" }]);
  });
});

describe("buildExportBundle", () => {
  it("pins the schema, strips ids/timestamps, coerces agents", () => {
    const b = buildExportBundle([
      { id: "tpl_x", name: "team", created_date: "old", updated_date: "old", agents: [{ role: "r", kind: "cline" }] },
    ]);
    expect(b.schema).toBe(TEMPLATE_SCHEMA_VERSION);
    expect(b.source).toBe("harness-ready");
    expect(typeof b.exported_at).toBe("string");
    expect(b.templates[0]).not.toHaveProperty("id");
    expect(b.templates[0]).not.toHaveProperty("created_date");
    expect(b.templates[0].name).toBe("team");
    expect(b.templates[0].agents[0].kind).toBe("pi");
  });
  it("is safe on empty/undefined input", () => {
    expect(buildExportBundle(undefined).templates).toEqual([]);
    expect(buildExportBundle([]).templates).toEqual([]);
  });
});

describe("downloadJSON", () => {
  it("is a no-op (does not throw) outside a DOM", () => {
    expect(() => downloadJSON("x.json", "{}")).not.toThrow();
  });
});

describe("parseImportPayload edges", () => {
  it("passes a bundle with no schema field through", () => {
    const r = parseImportPayload(JSON.stringify({ templates: [{ name: "a" }] }));
    expect(r.ok).toBe(true);
    expect(r.templates).toHaveLength(1);
  });
  it("accepts an empty array", () => {
    const r = parseImportPayload("[]");
    expect(r.ok).toBe(true);
    expect(r.templates).toEqual([]);
    expect(r.skipped).toEqual([]);
  });
  it("refuses an oversized bundle up front (MAX_IMPORT)", () => {
    const big = Array.from({ length: 501 }, (_, i) => ({ name: `t${i}` }));
    const r = parseImportPayload(JSON.stringify(big));
    expect(r.ok).toBe(false);
    expect(r.errors[0]).toMatch(/too many/);
  });
});
