// commandPalette.test.js — vitest suite for the pure commandPalette helpers.
// Covers: substring filtering (label + keywords), empty/whitespace queries,
// case-insensitivity, stable ordering, wrap-around index math edge cases,
// empty list, and no-match scenarios.

import { describe, it, expect } from "vitest";
import { filterCommands, moveIndex } from "./commandPalette";

// ---------------------------------------------------------------------------
// filterCommands
// ---------------------------------------------------------------------------

const CMDS = [
  { id: "new", label: "New Agent", keywords: ["spawn", "create"], hint: "⌘N", run: () => {} },
  { id: "broadcast", label: "Broadcast to all", keywords: ["send"], run: () => {} },
  { id: "delegate", label: "Delegate", run: () => {} },
  { id: "templates", label: "Templates", keywords: ["launch", "preset"], run: () => {} },
  { id: "monitoring", label: "Go to Monitoring", keywords: ["metrics", "fleet"], run: () => {} },
];

describe("filterCommands", () => {
  it("returns all commands when query is empty", () => {
    expect(filterCommands("", CMDS)).toEqual(CMDS);
  });

  it("returns all commands when query is whitespace-only", () => {
    expect(filterCommands("   ", CMDS)).toEqual(CMDS);
  });

  it("returns all commands when query is null/undefined", () => {
    expect(filterCommands(null, CMDS)).toEqual(CMDS);
    expect(filterCommands(undefined, CMDS)).toEqual(CMDS);
  });

  it("matches on label (case-insensitive)", () => {
    const result = filterCommands("BROADCAST", CMDS);
    expect(result).toHaveLength(1);
    expect(result[0].id).toBe("broadcast");
  });

  it("matches on label substring", () => {
    const result = filterCommands("agent", CMDS);
    expect(result).toHaveLength(1);
    expect(result[0].id).toBe("new");
  });

  it("matches on keywords", () => {
    const result = filterCommands("spawn", CMDS);
    expect(result).toHaveLength(1);
    expect(result[0].id).toBe("new");
  });

  it("matches keyword case-insensitively", () => {
    const result = filterCommands("METRICS", CMDS);
    expect(result).toHaveLength(1);
    expect(result[0].id).toBe("monitoring");
  });

  it("returns multiple matches in stable order", () => {
    // "to" appears in "Broadcast to all" and "Go to Monitoring" labels
    const result = filterCommands("to", CMDS);
    expect(result).toHaveLength(2);
    expect(result[0].id).toBe("broadcast");
    expect(result[1].id).toBe("monitoring");
  });

  it("returns empty array when nothing matches", () => {
    const result = filterCommands("xyzzy", CMDS);
    expect(result).toEqual([]);
  });

  it("returns empty array for empty input list", () => {
    expect(filterCommands("anything", [])).toEqual([]);
  });

  it("returns empty array for empty input list with empty query", () => {
    expect(filterCommands("", [])).toEqual([]);
  });

  it("preserves original order across multiple matches", () => {
    // "e" appears in: "New Agent" (no — wait, "Agent" has 'e'), "Delegate" (yes), "Templates" (yes)
    // Actually: "New Agent" has 'e' in "Agent", "Delegate" has 'e', "Templates" has 'e'
    const result = filterCommands("e", CMDS);
    const ids = result.map((c) => c.id);
    // All three should appear in their original order
    expect(ids.indexOf("new")).toBeLessThan(ids.indexOf("delegate"));
    expect(ids.indexOf("delegate")).toBeLessThan(ids.indexOf("templates"));
  });

  it("handles commands with no keywords array", () => {
    const noKw = [{ id: "a", label: "Alpha", run: () => {} }];
    expect(filterCommands("alpha", noKw)).toHaveLength(1);
    expect(filterCommands("beta", noKw)).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// moveIndex
// ---------------------------------------------------------------------------

describe("moveIndex", () => {
  describe("empty list", () => {
    it("returns -1 for count 0 regardless of direction", () => {
      expect(moveIndex(1, 0, -1)).toBe(-1);
      expect(moveIndex(-1, 0, -1)).toBe(-1);
      expect(moveIndex(1, 0, 0)).toBe(-1);
    });
  });

  describe("from no selection (current = -1)", () => {
    it("down moves to first item (index 0)", () => {
      expect(moveIndex(1, 5, -1)).toBe(0);
    });

    it("up moves to last item", () => {
      expect(moveIndex(-1, 5, -1)).toBe(4);
    });

    it("works with a single item", () => {
      expect(moveIndex(1, 1, -1)).toBe(0);
      expect(moveIndex(-1, 1, -1)).toBe(0);
    });
  });

  describe("wrap-around", () => {
    it("down at last item wraps to 0", () => {
      expect(moveIndex(1, 5, 4)).toBe(0);
    });

    it("up at first item wraps to last", () => {
      expect(moveIndex(-1, 5, 0)).toBe(4);
    });

    it("wraps correctly with a single item", () => {
      expect(moveIndex(1, 1, 0)).toBe(0);
      expect(moveIndex(-1, 1, 0)).toBe(0);
    });
  });

  describe("normal navigation", () => {
    it("down increments by 1", () => {
      expect(moveIndex(1, 5, 0)).toBe(1);
      expect(moveIndex(1, 5, 2)).toBe(3);
    });

    it("up decrements by 1", () => {
      expect(moveIndex(-1, 5, 4)).toBe(3);
      expect(moveIndex(-1, 5, 2)).toBe(1);
    });
  });

  describe("two-item list", () => {
    it("navigates correctly", () => {
      expect(moveIndex(1, 2, -1)).toBe(0);
      expect(moveIndex(1, 2, 0)).toBe(1);
      expect(moveIndex(1, 2, 1)).toBe(0); // wrap
      expect(moveIndex(-1, 2, 0)).toBe(1); // wrap
      expect(moveIndex(-1, 2, 1)).toBe(0);
    });
  });
});
