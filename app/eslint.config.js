// Flat ESLint config for the frontend (app/src). This is the FIRST JS lint in the repo —
// its job is to catch typo'd invoke() names, undefined vars, and dead bindings that vitest
// (which only exercises the extracted *-core.js modules) can never see in the 7500-line main.js.
//
// no-undef is an ERROR (a reference to an undeclared global is almost always a bug — a typo'd
// Tauri command or a renamed function). no-unused-vars is a WARN (dead bindings are noise, not
// breakage). Pre-existing violations do NOT fail the build: the CI `lint` step is advisory
// (continue-on-error) until the count reaches zero — see .github/workflows/ci.yml.
import js from "@eslint/js";
import globals from "globals";

export default [
  // Vendored, minified third-party bundles (xterm + addons) are not ours to lint.
  { ignores: ["**/vendor/**", "node_modules/**", "playwright-report/**", "test-results/**"] },
  js.configs.recommended,
  {
    files: ["src/**/*.js"],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      globals: { ...globals.browser },
    },
    rules: {
      // The ONE hard signal we want today: a reference to an undeclared global (typo'd Tauri
      // command, renamed helper). This is already clean across app/src — keep it that way.
      "no-undef": "error",
      // Pre-existing patterns in the 7500-line main.js — demoted to WARN so the lint stays
      // green (advisory) instead of drowning the new no-undef gate in legacy noise. Burn these
      // down over time, then promote back to "error":
      "no-unused-vars": "warn",
      "no-empty": "warn",          // deliberate `catch (_) {}` swallows throughout main.js
      "no-control-regex": "warn",  // ANSI/control-char stripping regexes
    },
  },
  {
    // Test + tooling files also see the node + vitest environment.
    files: ["src/**/*.test.js", "*.config.js"],
    languageOptions: {
      globals: { ...globals.node, ...globals.browser },
    },
  },
];
