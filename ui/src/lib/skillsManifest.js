// skillsManifest.js — PURE library for parsing SKILL.md YAML frontmatter and
// building a sorted, defensive manifest. No IO, no side effects.
// R-SKILLS-OSS: the catalog is a static build-time snapshot (skills-raw.json);
// this module normalizes it at runtime for the Skills page.

/**
 * Parse YAML-like frontmatter text into { name, description }.
 * Tolerates:
 *  - missing fields → ""
 *  - quoted values (single or double) → stripped
 *  - folded-block style `description: >` (joins continuation lines)
 *  - malformed / empty / null input → { name: "", description: "" }
 *
 * Never throws.
 *
 * @param {string} text — the raw frontmatter block (between --- markers)
 * @returns {{ name: string, description: string }}
 */
export function parseSkillFrontmatter(text) {
  const empty = { name: "", description: "" };
  if (typeof text !== "string" || text.trim() === "") return empty;

  const lines = text.split("\n");
  let name = "";
  let description = "";
  let currentKey = null;
  let folded = false; // true when last key used `>` folded block

  const stripQuotes = (v) => {
    const trimmed = v.trim();
    if (
      (trimmed.startsWith('"') && trimmed.endsWith('"')) ||
      (trimmed.startsWith("'") && trimmed.endsWith("'"))
    ) {
      return trimmed.slice(1, -1).trim();
    }
    return trimmed;
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];

    // Continuation line for a folded-block value (indented, currentKey set, folded flag)
    if (currentKey && folded && /^\s+/.test(line) && line.trim() !== "") {
      const cont = line.trim();
      if (currentKey === "name") {
        name += (name ? " " : "") + cont;
      } else if (currentKey === "description") {
        description += (description ? " " : "") + cont;
      }
      continue;
    }

    // Non-continuation: reset folded state
    folded = false;

    // Try to match a top-level key: value pair
    const m = line.match(/^(name|description)\s*:\s*(.*)/i);
    if (!m) continue;

    const key = m[1].toLowerCase();
    const rawVal = m[2];
    currentKey = key;

    // Folded-block indicator: `>` or `>-` or `>|`
    if (/^>\s*$/.test(rawVal.trim()) || rawVal.trim() === ">-" || rawVal.trim() === ">|") {
      folded = true;
      continue; // value comes on next indented lines
    }

    // Inline value
    const val = stripQuotes(rawVal);
    if (key === "name") {
      name = val;
    } else if (key === "description") {
      description = val;
    }
  }

  return { name: name.trim(), description: description.trim() };
}

/**
 * Build a sorted manifest from the skills-raw.json shape.
 * Each input element: { source: string, text: string }
 * Returns: [{ id, name, description, source }] sorted by name (ascending),
 * SKIPPING entries whose parsed name is empty (defensive — never throws on
 * malformed input).
 *
 * @param {Array<{ source: string, text: string }>} raw
 * @returns {Array<{ id: string, name: string, description: string, source: string }>}
 */
export function buildSkillsManifest(raw) {
  if (!Array.isArray(raw)) return [];

  const entries = [];
  for (let i = 0; i < raw.length; i++) {
    const item = raw[i];
    if (!item || typeof item !== "object") continue;
    const { name, description } = parseSkillFrontmatter(
      typeof item.text === "string" ? item.text : ""
    );
    if (!name) continue; // skip unparseable — R3/R4: never invent
    entries.push({
      id: `skill-${i}-${name.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`,
      name,
      description,
      source: typeof item.source === "string" ? item.source : "",
    });
  }

  entries.sort((a, b) => a.name.localeCompare(b.name));
  return entries;
}
