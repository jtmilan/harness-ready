import { defineConfig } from "vitest/config";

// Unit tests for pure frontend logic (presets store). Node env — no DOM; tests that
// need localStorage shim globalThis.localStorage themselves. Scoped to src/ so vitest
// never wanders into sibling git worktrees.
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.js"],
  },
});
