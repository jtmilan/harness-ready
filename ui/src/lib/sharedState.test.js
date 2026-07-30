// Pure helpers — no DOM, no bridge. Defensive on unknown modes so the header
// chip cannot crash on a future backend shape change.
import { describe, it, expect } from "vitest";
import { sharedStateLabel, isWritable } from "@/lib/sharedState";

describe("sharedStateLabel", () => {
  it("labels the explicit read-write mode", () => {
    expect(sharedStateLabel("read-write")).toEqual({
      text: "SHARED-STATE: read-write",
      tone: "readwrite",
    });
  });

  it("labels the explicit read-only mode", () => {
    expect(sharedStateLabel("read-only")).toEqual({
      text: "SHARED-STATE: read-only",
      tone: "readonly",
    });
  });

  it("defaults to read-only for null (no backend source yet)", () => {
    expect(sharedStateLabel(null)).toEqual({
      text: "SHARED-STATE: read-only",
      tone: "readonly",
    });
  });

  it("defaults to read-only for undefined (no backend source yet)", () => {
    expect(sharedStateLabel(undefined)).toEqual({
      text: "SHARED-STATE: read-only",
      tone: "readonly",
    });
  });

  it("defaults to read-only for garbage string input", () => {
    expect(sharedStateLabel("banana")).toEqual({
      text: "SHARED-STATE: read-only",
      tone: "readonly",
    });
    expect(sharedStateLabel("READ-WRITE")).toEqual({
      text: "SHARED-STATE: read-only",
      tone: "readonly",
    });
    expect(sharedStateLabel("")).toEqual({
      text: "SHARED-STATE: read-only",
      tone: "readonly",
    });
  });

  it("defaults to read-only for non-string inputs", () => {
    expect(sharedStateLabel(42)).toEqual({
      text: "SHARED-STATE: read-only",
      tone: "readonly",
    });
    expect(sharedStateLabel({ mode: "read-write" })).toEqual({
      text: "SHARED-STATE: read-only",
      tone: "readonly",
    });
    expect(sharedStateLabel(true)).toEqual({
      text: "SHARED-STATE: read-only",
      tone: "readonly",
    });
  });

  it("never throws on arbitrary input", () => {
    expect(() => sharedStateLabel(Symbol("x"))).not.toThrow();
    expect(() => sharedStateLabel(() => {})).not.toThrow();
  });
});

describe("isWritable", () => {
  it("is true only for the explicit read-write mode", () => {
    expect(isWritable("read-write")).toBe(true);
  });

  it("is false for read-only", () => {
    expect(isWritable("read-only")).toBe(false);
  });

  it("is false for null and undefined", () => {
    expect(isWritable(null)).toBe(false);
    expect(isWritable(undefined)).toBe(false);
  });

  it("is false for garbage strings and non-strings", () => {
    expect(isWritable("banana")).toBe(false);
    expect(isWritable("READ-WRITE")).toBe(false);
    expect(isWritable("")).toBe(false);
    expect(isWritable(0)).toBe(false);
    expect(isWritable({})).toBe(false);
  });
});
