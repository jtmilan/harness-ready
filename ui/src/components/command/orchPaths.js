// F-ORCH-2 foundation (R-ORCH). Owned-paths are ADVISORY operator intent — the backend does NOT
// enforce them (worktrees already isolate; an "enforce" toggle would be a fake affordance, R3 —
// see docs/proposals/SYNTHESIS.md §3 V-B1 / DESIGN-BRIEF §4.1). The collision check is a
// NON-BLOCKING WARNING so two writers don't *believe* they own the same files. Pure + unit-
// tested here; the warning UI ships where multi-row context exists (template / orchestrate
// preview), on the templates-stacked branch.

/** Normalize a comma/newline-separated owned-paths string → trimmed, de-duped, non-empty list. */
export function parseOwnedPaths(value) {
  if (typeof value !== "string") return [];
  return Array.from(new Set(value.split(/[\n,]/).map((s) => s.trim()).filter(Boolean)));
}

// Conservative glob-ish overlap: exact, prefix-segment, or a trailing /** (or /*) wildcard that
// contains the other. We warn on ambiguity rather than miss a real clash.
// LIMITATION: mid-pattern globs (e.g. `src/**/*.ts`, `*.tsx`) are NOT expanded — they fall through
// to the exact/prefix branches. Intentional for v1; swap in micromatch when F-ORCH-2 ships the UI.
function pathsOverlap(a, b) {
  if (a === b) return true;
  const norm = (p) => p.replace(/\/+$/, "");
  const na = norm(a);
  const nb = norm(b);
  const contains = (glob, path) => {
    if (glob.endsWith("/**")) {
      const root = glob.slice(0, -3);
      return path === root || path.startsWith(root + "/");
    }
    if (glob.endsWith("/*")) return path.startsWith(glob.slice(0, -2) + "/");
    return false;
  };
  if (na.startsWith(nb + "/") || nb.startsWith(na + "/")) return true;
  if (contains(na, nb) || contains(nb, na)) return true;
  return false;
}

// Read-mostly roles are excluded from collision warnings (a reviewer reading everything is not a
// clash). Unknown/empty role → assume writer (conservative: better a false warning than a missed
// merge conflict).
const READER_ROLES = new Set(["reviewer", "scout", "coordinator", "observer", "reader"]);
function isWriter(role) {
  if (role == null || String(role).trim() === "") return true;
  return !READER_ROLES.has(String(role).trim().toLowerCase());
}

/**
 * @param {{ id?: string|number, role?: string, ownedPaths?: string[]|string }[]} rows
 * @returns {{ a: string|number, b: string|number, paths: string[] }[]}
 *   Non-blocking collision set between writer rows whose owned-paths overlap. Advisory only.
 */
export function detectOwnedPathCollisions(rows) {
  const list = Array.isArray(rows) ? rows : [];
  const collisions = [];
  for (let i = 0; i < list.length; i++) {
    for (let j = i + 1; j < list.length; j++) {
      const ri = list[i] || {};
      const rj = list[j] || {};
      if (!isWriter(ri.role) || !isWriter(rj.role)) continue;
      const pi = Array.isArray(ri.ownedPaths) ? ri.ownedPaths : parseOwnedPaths(ri.ownedPaths);
      const pj = Array.isArray(rj.ownedPaths) ? rj.ownedPaths : parseOwnedPaths(rj.ownedPaths);
      const shared = [];
      for (const a of pi) for (const b of pj) if (pathsOverlap(a, b)) shared.push(`${a} ∩ ${b}`);
      if (shared.length) collisions.push({ a: ri.id ?? i, b: rj.id ?? j, paths: shared });
    }
  }
  return collisions;
}
