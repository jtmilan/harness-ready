import { defineConfig } from "vitest/config";

// Unit tests for pure frontend logic (layout geometry/tree). Node env — no DOM; the
// two existing files under src/lib/layout/__tests__/ are pure math. Scoped to src/
// so vitest never wanders into sibling git worktrees (this repo has many).
// include: `src/**/*.test.js` matches both colocated `*.test.js` and
// `__tests__/*.test.js` (verified against geometry.test.js + tree.test.js).
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.js"],
  },
});
