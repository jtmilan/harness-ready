import { beforeEach, describe, expect, it } from "vitest";
import {
  BUILTIN_PRESETS,
  KNOWN_HARNESSES,
  STORAGE_KEY,
  isKnownHarness,
  normalizePreset,
  listPresets,
  getPreset,
  savePreset,
  deletePreset,
  getActivePresetId,
  setActivePresetId,
  expandHarnesses,
} from "./presets.js";

// vitest 'node' env has no DOM — give the module a fresh in-memory localStorage per test.
function makeLocalStorage() {
  const m = new Map();
  return {
    getItem: (k) => (m.has(k) ? m.get(k) : null),
    setItem: (k, v) => m.set(k, String(v)),
    removeItem: (k) => m.delete(k),
    clear: () => m.clear(),
  };
}
beforeEach(() => { globalThis.localStorage = makeLocalStorage(); });

describe("invariant: count === harnesses.length", () => {
  it("derives count from harnesses, ignoring a wrong incoming count", () => {
    const p = normalizePreset({ name: "x", harnesses: ["claude", "cursor"], count: 99 });
    expect(p.count).toBe(2);
  });
  it("rejects a preset with no usable harnesses (returns null)", () => {
    expect(normalizePreset({ name: "empty", harnesses: [] })).toBeNull();
    expect(normalizePreset({ name: "junk", harnesses: [1, null, ""] })).toBeNull();
    expect(normalizePreset(null)).toBeNull();
  });
  it("savePreset refuses an empty harness list", () => {
    expect(() => savePreset({ name: "n", harnesses: [] })).toThrow(/at least one harness/);
  });
});

describe("built-ins", () => {
  it("are present on a fresh (empty) store, builtins first", () => {
    const all = listPresets();
    expect(all.length).toBe(BUILTIN_PRESETS.length);
    expect(all.every((p) => p.builtin)).toBe(true);
    expect(all[0].id).toMatch(/^builtin-/);
  });
  it("every built-in satisfies the invariant and uses known harnesses", () => {
    for (const b of listPresets()) {
      expect(b.count).toBe(b.harnesses.length);
      expect(b.harnesses.every(isKnownHarness)).toBe(true);
      expect(b.unspawnable).toBe(false);
    }
  });
  it("cannot be deleted or overwritten", () => {
    expect(() => deletePreset("builtin-4-claude")).toThrow(/cannot be deleted/);
    expect(() => savePreset({ id: "builtin-4-claude", name: "hijack", harnesses: ["bash"] })).toThrow(/read-only/);
  });
});

describe("user preset CRUD", () => {
  it("saves and reads back a mixed preset; persists across a reload", () => {
    const saved = savePreset({ name: "Mix", harnesses: ["claude", "cursor"] });
    expect(saved.id).toMatch(/^preset-/);
    expect(saved.count).toBe(2);
    // a "reload" = re-read from the same storage
    const again = getPreset(saved.id);
    expect(again).not.toBeNull();
    expect(again.harnesses).toEqual(["claude", "cursor"]);
    expect(again.builtin).toBe(false);
    expect(listPresets().length).toBe(BUILTIN_PRESETS.length + 1);
  });
  it("rejects an unknown harness on save (strict), accepts it on lenient read", () => {
    expect(() => savePreset({ name: "bad", harnesses: ["gemini"] })).toThrow(/unknown harness: gemini/);
    // a preset stored before a harness was removed still loads, flagged unspawnable
    const stored = { v: 1, presets: [{ id: "preset-legacy", name: "Legacy", harnesses: ["claude", "gemini"] }] };
    globalThis.localStorage.setItem(STORAGE_KEY, JSON.stringify(stored));
    const legacy = getPreset("preset-legacy");
    expect(legacy).not.toBeNull();
    expect(legacy.unspawnable).toBe(true);
    expect(legacy.count).toBe(2);
  });
  it("delete removes a user preset and returns true; false when absent", () => {
    const { id } = savePreset({ name: "Doomed", harnesses: ["bash"] });
    expect(deletePreset(id)).toBe(true);
    expect(getPreset(id)).toBeNull();
    expect(deletePreset("preset-nonexistent")).toBe(false);
  });
});

describe("robustness + active pointer", () => {
  it("listPresets survives corrupt storage (returns built-ins, no throw)", () => {
    globalThis.localStorage.setItem(STORAGE_KEY, "{not json");
    const all = listPresets();
    expect(all.length).toBe(BUILTIN_PRESETS.length);
  });
  it("active-preset pointer get/set/clear", () => {
    expect(getActivePresetId()).toBeNull();
    setActivePresetId("builtin-4-cursor");
    expect(getActivePresetId()).toBe("builtin-4-cursor");
    setActivePresetId(null);
    expect(getActivePresetId()).toBeNull();
  });
  it("deleting the active preset clears the pointer", () => {
    const { id } = savePreset({ name: "Active", harnesses: ["claude"] });
    setActivePresetId(id);
    deletePreset(id);
    expect(getActivePresetId()).toBeNull();
  });
  it("KNOWN_HARNESSES matches the app's harnesses (display order)", () => {
    expect(KNOWN_HARNESSES).toEqual(["claude", "codex", "commandcode", "cursor", "opencode", "pi", "grok", "bash"]);
  });
});

describe("expandHarnesses (add-agent harness chips → per-pane round-robin)", () => {
  it("single harness fills every pane", () => {
    expect(expandHarnesses(["claude"], 4)).toEqual(["claude", "claude", "claude", "claude"]);
  });
  it("two harnesses alternate across the count", () => {
    expect(expandHarnesses(["claude", "cursor"], 4)).toEqual(["claude", "cursor", "claude", "cursor"]);
  });
  it("preserves selection order (cursor first → cursor on p0)", () => {
    expect(expandHarnesses(["cursor", "claude"], 3)).toEqual(["cursor", "claude", "cursor"]);
  });
  it("three harnesses round-robin", () => {
    expect(expandHarnesses(["claude", "cursor", "bash"], 5))
      .toEqual(["claude", "cursor", "bash", "claude", "cursor"]);
  });
  it("count not divisible by the set length still terminates correctly", () => {
    expect(expandHarnesses(["claude", "cursor"], 3)).toEqual(["claude", "cursor", "claude"]);
  });
  it("drops unknown harness ids", () => {
    expect(expandHarnesses(["claude", "gemini", "cursor"], 4))
      .toEqual(["claude", "cursor", "claude", "cursor"]);
  });
  it("empty / invalid selection falls back to all-claude (never harness-less)", () => {
    expect(expandHarnesses([], 2)).toEqual(["claude", "claude"]);
    expect(expandHarnesses(null, 2)).toEqual(["claude", "claude"]);
    expect(expandHarnesses(["gemini"], 2)).toEqual(["claude", "claude"]);
  });
  it("count 0 → empty array", () => {
    expect(expandHarnesses(["claude", "cursor"], 0)).toEqual([]);
  });
});
