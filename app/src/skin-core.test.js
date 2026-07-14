// skin-core unit tests — pure logic, node env (no DOM, no localStorage; the stored
// value is passed in). Pins the boot precedence (?skin= > stored > default), the
// invalid-name fallback to "nothing", and the remove-on-default persistence decision.
import { describe, it, expect } from "vitest";
import {
  SKINS, SKIN_KEY, isValidSkin, normalizeSkin, resolveBootSkin, skinAttrFor, skinStorageOp,
} from "./skin-core.js";

describe("allowlist", () => {
  it("SKINS contains the default plus the five opt-in skins", () => {
    expect(SKINS).toEqual(["nothing", "aurora", "atelier", "phosphor", "precision", "liquid-glass"]);
    expect(SKIN_KEY).toBe("at_skin");
  });
  it("isValidSkin / normalizeSkin fall back to \"nothing\" on unknown names", () => {
    expect(isValidSkin("aurora")).toBe(true);
    expect(isValidSkin("neon")).toBe(false);
    expect(normalizeSkin("phosphor")).toBe("phosphor");
    expect(normalizeSkin("neon")).toBe("nothing");
    expect(normalizeSkin("")).toBe("nothing");
    expect(normalizeSkin(undefined)).toBe("nothing");
  });
});

describe("resolveBootSkin precedence", () => {
  it("?skin= URL param wins over the stored value", () => {
    expect(resolveBootSkin("?skin=aurora", "phosphor")).toBe("aurora");
  });
  it("falls back to the stored value when the param is absent or invalid", () => {
    expect(resolveBootSkin("", "precision")).toBe("precision");
    expect(resolveBootSkin("?skin=not-a-skin", "atelier")).toBe("atelier");
  });
  it("defaults to \"nothing\" when both are absent or invalid", () => {
    expect(resolveBootSkin("", null)).toBe("nothing");
    expect(resolveBootSkin("?skin=bogus", "also-bogus")).toBe("nothing");
    expect(resolveBootSkin(undefined, undefined)).toBe("nothing");
  });
  it("handles multi-param search strings", () => {
    expect(resolveBootSkin("?foo=1&skin=liquid-glass&bar=2", null)).toBe("liquid-glass");
  });
});

describe("skinAttrFor (delete-vs-set decision)", () => {
  it("default skin → null (REMOVE the data-skin attribute — bare :root)", () => {
    expect(skinAttrFor("nothing")).toBeNull();
    expect(skinAttrFor("garbage")).toBeNull(); // invalid collapses to default
  });
  it("a real skin → its own attribute value", () => {
    expect(skinAttrFor("aurora")).toBe("aurora");
    expect(skinAttrFor("liquid-glass")).toBe("liquid-glass");
  });
});

describe("skinStorageOp (localStorage-remove-on-default)", () => {
  it("default (and invalid → default) removes the persisted key", () => {
    expect(skinStorageOp("nothing")).toEqual({ action: "remove" });
    expect(skinStorageOp("not-a-skin")).toEqual({ action: "remove" });
  });
  it("a real skin persists its value", () => {
    expect(skinStorageOp("precision")).toEqual({ action: "set", value: "precision" });
  });
});
