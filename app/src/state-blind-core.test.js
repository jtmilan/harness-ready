import { describe, it, expect } from "vitest";
import {
  STATE_BLIND,
  STATE_BLIND_BADGE_LABEL,
  isStateBlind,
  stateBlindBadge,
} from "./state-blind-core.js";

describe("isStateBlind", () => {
  it("is true for the four inject:None harnesses", () => {
    for (const h of ["codex", "commandcode", "opencode", "pi"]) {
      expect(isStateBlind(h)).toBe(true);
    }
  });

  it("is false for state-reporting harnesses", () => {
    for (const h of ["claude", "cursor", "gemini", "bash", "sh", "zsh"]) {
      expect(isStateBlind(h)).toBe(false);
    }
  });

  it("is case-insensitive and trims whitespace", () => {
    expect(isStateBlind("Codex")).toBe(true);
    expect(isStateBlind("  OPENCODE  ")).toBe(true);
    expect(isStateBlind("PI")).toBe(true);
  });

  it("is false for null / undefined / empty (unknown harness → no badge)", () => {
    expect(isStateBlind(null)).toBe(false);
    expect(isStateBlind(undefined)).toBe(false);
    expect(isStateBlind("")).toBe(false);
    expect(isStateBlind("   ")).toBe(false);
    expect(isStateBlind("nonsense")).toBe(false);
  });

  it("exports the exact state-blind membership", () => {
    expect([...STATE_BLIND].sort()).toEqual(
      ["pi", "codex", "commandcode", "opencode"].sort(),
    );
  });
});

describe("stateBlindBadge", () => {
  it("returns null for a state-reporting harness (no badge painted)", () => {
    expect(stateBlindBadge("claude")).toBeNull();
    expect(stateBlindBadge("cursor")).toBeNull();
    expect(stateBlindBadge("")).toBeNull();
    expect(stateBlindBadge(null)).toBeNull();
  });

  it("returns a badge with the shared label for a state-blind harness", () => {
    const b = stateBlindBadge("codex");
    expect(b).not.toBeNull();
    expect(b.label).toBe(STATE_BLIND_BADGE_LABEL);
    expect(b.label).toBe("status not reported");
  });

  it("names the harness in the hover/aria title", () => {
    expect(stateBlindBadge("opencode").title).toContain("opencode");
    expect(stateBlindBadge("pi").title).toContain("pi");
  });

  it("still badges when case differs but preserves the original casing in the title", () => {
    const b = stateBlindBadge("Codex");
    expect(b).not.toBeNull();
    expect(b.title).toContain("Codex");
  });
});
