// skillsManifest.test.js — vitest suite for the pure skillsManifest helpers.
// Covers: clean frontmatter, folded description (>), missing fields,
// malformed/empty text, unparseable entries skipped, sorting, empty array.

import { describe, it, expect } from "vitest";
import { parseSkillFrontmatter, buildSkillsManifest } from "./skillsManifest";

// ---------------------------------------------------------------------------
// parseSkillFrontmatter
// ---------------------------------------------------------------------------

describe("parseSkillFrontmatter", () => {
  it("parses clean name and description", () => {
    const text = "name: my-skill\ndescription: A useful skill for things";
    expect(parseSkillFrontmatter(text)).toEqual({
      name: "my-skill",
      description: "A useful skill for things",
    });
  });

  it("strips single and double quotes from values", () => {
    const a = 'name: "quoted-name"\ndescription: \'quoted-desc\'';
    expect(parseSkillFrontmatter(a)).toEqual({
      name: "quoted-name",
      description: "quoted-desc",
    });
  });

  it("handles folded-block description (>)", () => {
    const text = [
      "name: folded-skill",
      "description: >",
      "  This is a long",
      "  description that spans",
      "  multiple lines.",
    ].join("\n");
    expect(parseSkillFrontmatter(text)).toEqual({
      name: "folded-skill",
      description: "This is a long description that spans multiple lines.",
    });
  });

  it("handles folded-block description with >- strip indicator", () => {
    const text = [
      "name: strip-skill",
      "description: >-",
      "  First line",
      "  second line",
    ].join("\n");
    expect(parseSkillFrontmatter(text)).toEqual({
      name: "strip-skill",
      description: "First line second line",
    });
  });

  it("returns empty strings when name is missing", () => {
    const text = "description: orphan description";
    expect(parseSkillFrontmatter(text)).toEqual({
      name: "",
      description: "orphan description",
    });
  });

  it("returns empty strings when description is missing", () => {
    const text = "name: name-only";
    expect(parseSkillFrontmatter(text)).toEqual({
      name: "name-only",
      description: "",
    });
  });

  it("returns empty strings for empty text", () => {
    expect(parseSkillFrontmatter("")).toEqual({ name: "", description: "" });
  });

  it("returns empty strings for null/undefined input", () => {
    expect(parseSkillFrontmatter(null)).toEqual({ name: "", description: "" });
    expect(parseSkillFrontmatter(undefined)).toEqual({ name: "", description: "" });
  });

  it("returns empty strings for whitespace-only text", () => {
    expect(parseSkillFrontmatter("   \n  \n  ")).toEqual({ name: "", description: "" });
  });

  it("returns empty strings for gibberish with no key: value lines", () => {
    expect(parseSkillFrontmatter("hello world\nno keys here")).toEqual({
      name: "",
      description: "",
    });
  });

  it("handles extra unrelated frontmatter lines gracefully", () => {
    const text = [
      "version: 1.0",
      "name: buried-skill",
      "author: someone",
      "description: found among noise",
      "tags: [a, b]",
    ].join("\n");
    expect(parseSkillFrontmatter(text)).toEqual({
      name: "buried-skill",
      description: "found among noise",
    });
  });

  it("is case-insensitive for key matching", () => {
    const text = "Name: Mixed\ndescription: lower";
    expect(parseSkillFrontmatter(text)).toEqual({
      name: "Mixed",
      description: "lower",
    });
  });
});

// ---------------------------------------------------------------------------
// buildSkillsManifest
// ---------------------------------------------------------------------------

describe("buildSkillsManifest", () => {
  it("returns empty array for empty input", () => {
    expect(buildSkillsManifest([])).toEqual([]);
  });

  it("returns empty array for non-array input", () => {
    expect(buildSkillsManifest(null)).toEqual([]);
    expect(buildSkillsManifest(undefined)).toEqual([]);
    expect(buildSkillsManifest("nope")).toEqual([]);
  });

  it("builds a manifest from valid entries", () => {
    const raw = [
      { source: ".claude/skills/alpha", text: "name: Alpha\ndescription: First skill" },
      { source: ".claude/skills/beta", text: "name: Beta\ndescription: Second skill" },
    ];
    const result = buildSkillsManifest(raw);
    expect(result).toHaveLength(2);
    expect(result[0].name).toBe("Alpha");
    expect(result[1].name).toBe("Beta");
  });

  it("sorts entries by name ascending", () => {
    const raw = [
      { source: ".claude/skills/zeta", text: "name: Zeta\ndescription: last" },
      { source: ".claude/skills/alpha", text: "name: Alpha\ndescription: first" },
      { source: ".claude/skills/mid", text: "name: Middle\ndescription: mid" },
    ];
    const result = buildSkillsManifest(raw);
    expect(result.map((r) => r.name)).toEqual(["Alpha", "Middle", "Zeta"]);
  });

  it("skips entries with empty/missing name", () => {
    const raw = [
      { source: ".claude/skills/good", text: "name: Good\ndescription: valid" },
      { source: ".claude/skills/noname", text: "description: no name here" },
      { source: ".claude/skills/bad", text: "garbage text" },
    ];
    const result = buildSkillsManifest(raw);
    expect(result).toHaveLength(1);
    expect(result[0].name).toBe("Good");
  });

  it("skips null/undefined items in the array", () => {
    const raw = [
      null,
      undefined,
      { source: ".claude/skills/ok", text: "name: Ok\ndescription: fine" },
      42,
    ];
    const result = buildSkillsManifest(raw);
    expect(result).toHaveLength(1);
    expect(result[0].name).toBe("Ok");
  });

  it("handles entries with missing text field", () => {
    const raw = [{ source: ".claude/skills/notext" }];
    const result = buildSkillsManifest(raw);
    expect(result).toEqual([]); // no name → skipped
  });

  it("generates stable ids from index + slugified name", () => {
    const raw = [
      { source: ".claude/skills/a", text: "name: My Cool Skill\ndescription: x" },
    ];
    const result = buildSkillsManifest(raw);
    expect(result[0].id).toBe("skill-0-my-cool-skill");
  });

  it("preserves source path in output", () => {
    const raw = [
      { source: ".claude/skills/foo", text: "name: Foo\ndescription: bar" },
    ];
    const result = buildSkillsManifest(raw);
    expect(result[0].source).toBe(".claude/skills/foo");
  });

  it("handles folded-block descriptions end-to-end", () => {
    const raw = [
      {
        source: ".claude/skills/folded",
        text: "name: Folded\ndescription: >\n  A multi-line\n  description here.",
      },
    ];
    const result = buildSkillsManifest(raw);
    expect(result[0].description).toBe("A multi-line description here.");
  });

  it("returns empty for an all-malformed input array", () => {
    const raw = [
      { source: "a", text: "no keys" },
      { source: "b", text: "" },
      { source: "c", text: null },
    ];
    expect(buildSkillsManifest(raw)).toEqual([]);
  });
});
