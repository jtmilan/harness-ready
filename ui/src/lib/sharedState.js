// Pure helpers for the R-MCP-RW shared-state mode indicator (Phase 7).
//
// The read-write shared task/context layer is gated-OFF by default — see
// docs/REQUIRES-HUMAN-DESIGN-shared-state-write.md. Until a backend exposes
// the mode via `bridge.getSharedStateMode()` (NEEDS-BACKEND; contract noted in
// SharedStateBadge.jsx), callers pass `undefined` and these helpers render the
// safe default: read-only.
//
// These functions NEVER throw. Any unexpected input falls through to the
// read-only default so a UI chip built on them cannot crash the header.

/**
 * @typedef {"readonly" | "readwrite" | "unknown"} SharedStateTone
 */

/**
 * @typedef {{ text: string, tone: SharedStateTone }} SharedStateLabel
 */

const READONLY_LABEL = /** @type {const} */ ({
  text: "SHARED-STATE: read-only",
  tone: "readonly",
});

const READWRITE_LABEL = /** @type {const} */ ({
  text: "SHARED-STATE: read-write",
  tone: "readwrite",
});

/**
 * Map a backend mode flag to a human-readable label + tone.
 *
 * The mode value is the proposed return shape of a future
 * `bridge.getSharedStateMode()` — `'read-only' | 'read-write'`. Anything else
 * (null, undefined, unknown string, non-string) safely falls through to the
 * read-only default, which is the real gated-OFF state today.
 *
 * @param {unknown} mode
 * @returns {SharedStateLabel}
 */
export function sharedStateLabel(mode) {
  if (mode === "read-write") return READWRITE_LABEL;
  if (mode === "read-only") return READONLY_LABEL;
  // null / undefined / garbage / unknown enum → safe default
  return READONLY_LABEL;
}

/**
 * True only when the backend has explicitly reported the read-write mode.
 * Any other value (including "read-only", null, undefined, garbage) is false.
 *
 * @param {unknown} mode
 * @returns {boolean}
 */
export function isWritable(mode) {
  return mode === "read-write";
}
