// external-spawn-core unit tests — pure logic, node env (no DOM). Covers externalSpawnCap
// (payload `cap` → effective FE clamp: fallback 8, floor 1, hard ceiling 16) and
// expandExternalPanes (groups → parallel arrays, first-groups-win truncation at the cap).
import { describe, it, expect } from "vitest";
import {
  EXTERNAL_SPAWN_DEFAULT_CAP,
  EXTERNAL_SPAWN_HARD_CEILING,
  externalSpawnCap,
  expandExternalPanes,
} from "./external-spawn-core.js";

describe("externalSpawnCap", () => {
  it("falls back to the default (8) when the payload carries no cap (old backends)", () => {
    expect(externalSpawnCap(undefined)).toBe(EXTERNAL_SPAWN_DEFAULT_CAP);
    expect(externalSpawnCap({})).toBe(8);
    expect(externalSpawnCap({ cap: 0 })).toBe(8);
    expect(externalSpawnCap({ cap: "garbage" })).toBe(8);
  });
  it("uses the config-resolved payload cap, coercing numeric strings and floats", () => {
    expect(externalSpawnCap({ cap: 12 })).toBe(12);
    expect(externalSpawnCap({ cap: "12" })).toBe(12);
    expect(externalSpawnCap({ cap: 8.9 })).toBe(8);
    expect(externalSpawnCap({ cap: 1 })).toBe(1);
  });
  it("never trusts a payload past the hard ceiling (16) or below 1", () => {
    expect(externalSpawnCap({ cap: 99 })).toBe(EXTERNAL_SPAWN_HARD_CEILING);
    expect(externalSpawnCap({ cap: 16 })).toBe(16);
    expect(externalSpawnCap({ cap: -3 })).toBe(1);
  });
});

describe("expandExternalPanes", () => {
  it("expands groups within the cap: requested === count, no truncation", () => {
    const r = expandExternalPanes(
      [{ harness: "claude", role: "builder", count: 2 }, { harness: "codex", count: 1 }],
      8
    );
    expect(r.harnesses).toEqual(["claude", "claude", "codex"]);
    expect(r.requested).toBe(3);
    expect(r.harnesses.length).toBe(3);
    expect(r.truncated).toBe(false);
  });
  it("clamps past the cap first-groups-win, preserving the requested total", () => {
    const r = expandExternalPanes(
      [{ harness: "claude", count: 3 }, { harness: "codex", count: 3 }],
      4
    );
    expect(r.harnesses).toEqual(["claude", "claude", "claude", "codex"]);
    expect(r.requested).toBe(6);
    expect(r.truncated).toBe(true);
  });
  it("defaults harness claude / role none / model undefined; count 0 or missing → 1", () => {
    const r = expandExternalPanes([{ count: 0 }, {}], 8);
    expect(r.harnesses).toEqual(["claude", "claude"]);
    expect(r.roles).toEqual(["none", "none"]);
    expect(r.models).toEqual([undefined, undefined]);
    expect(r.requested).toBe(2);
  });
  it("keeps the parallel arrays equal length", () => {
    const r = expandExternalPanes(
      [{ harness: "claude", role: "builder", model: "opus", count: 5 }],
      3
    );
    expect(r.roles.length).toBe(r.harnesses.length);
    expect(r.models.length).toBe(r.harnesses.length);
    expect(r.harnesses.length).toBe(3);
  });
  it("empty groups → empty arrays, nothing requested, not truncated", () => {
    const r = expandExternalPanes([], 8);
    expect(r.harnesses).toEqual([]);
    expect(r.requested).toBe(0);
    expect(r.truncated).toBe(false);
  });
  it("a raised cap (16) admits 16 panes", () => {
    const r = expandExternalPanes([{ harness: "claude", count: 16 }], 16);
    expect(r.harnesses.length).toBe(16);
    expect(r.truncated).toBe(false);
  });
});
