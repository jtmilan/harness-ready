import { describe, it, expect } from "vitest";
import { parseOwnedPaths, detectOwnedPathCollisions } from "@/components/command/orchPaths";

describe("parseOwnedPaths", () => {
  it("splits on commas + newlines, trims, de-dupes, drops empties", () => {
    expect(parseOwnedPaths(" src/a ,src/b\n src/a , ,")).toEqual(["src/a", "src/b"]);
    expect(parseOwnedPaths("")).toEqual([]);
    expect(parseOwnedPaths(null)).toEqual([]);
  });
});

describe("detectOwnedPathCollisions", () => {
  it("flags exact + prefix + glob overlap between two writers", () => {
    const c = detectOwnedPathCollisions([
      { id: "p1", role: "builder", ownedPaths: ["src/auth", "src/**"] },
      { id: "p2", role: "builder", ownedPaths: ["src/auth/login.ts", "docs/x"] },
    ]);
    expect(c).toHaveLength(1);
    expect(c[0].a).toBe("p1");
    expect(c[0].b).toBe("p2");
    expect(c[0].paths.length).toBeGreaterThanOrEqual(2); // exact-ish via prefix + glob both match
  });
  it("does NOT flag disjoint paths", () => {
    const c = detectOwnedPathCollisions([
      { id: "p1", role: "builder", ownedPaths: ["src/auth"] },
      { id: "p2", role: "builder", ownedPaths: ["src/billing"] },
    ]);
    expect(c).toEqual([]);
  });
  it("does NOT flag a substring that is not a path segment (src/auth vs src/authentication)", () => {
    const c = detectOwnedPathCollisions([
      { id: "p1", role: "builder", ownedPaths: ["src/auth"] },
      { id: "p2", role: "builder", ownedPaths: ["src/authentication"] },
    ]);
    expect(c).toEqual([]);
  });
  it("excludes read-mostly roles (reviewer/scout) from warnings", () => {
    const c = detectOwnedPathCollisions([
      { id: "p1", role: "builder", ownedPaths: ["src/**"] },
      { id: "p2", role: "reviewer", ownedPaths: ["src/**"] },
      { id: "p3", role: "scout", ownedPaths: ["src/**"] },
    ]);
    expect(c).toEqual([]);
  });
  it("treats unknown/empty role as a writer (conservative)", () => {
    const c = detectOwnedPathCollisions([
      { id: "p1", ownedPaths: ["src/x"] },
      { id: "p2", role: "", ownedPaths: ["src/x"] },
    ]);
    expect(c).toHaveLength(1);
  });
  it("accepts ownedPaths as a raw string and ignores non-array/non-string noise", () => {
    const c = detectOwnedPathCollisions([
      { id: "p1", role: "builder", ownedPaths: "src/x, src/y" },
      { id: "p2", role: "builder", ownedPaths: "src/y" },
    ]);
    expect(c).toHaveLength(1);
    expect(c[0].paths[0]).toContain("src/y");
  });
});
