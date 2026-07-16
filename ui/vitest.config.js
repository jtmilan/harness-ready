import path from "path";
import { fileURLToPath } from "url";
import { defineConfig } from "vitest/config";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

// Unit tests for pure frontend logic (layout geometry/tree + wave-6 bridge/store
// contracts). Node env — no DOM. Scoped to src/ so vitest never wanders into
// sibling git worktrees (this repo has many).
// include: `src/**/*.test.js` matches both colocated `*.test.js` and
// `__tests__/*.test.js` (verified against geometry.test.js + tree.test.js).
//
// resolve.alias `@` MUST stay in lockstep with vite.config.js (same
// path.resolve(__dirname, './src')). Drift breaks either `vitest run` or
// `vite build` imports of `@/lib/...` — edit both or neither.
export default defineConfig({
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.js"],
  },
});
