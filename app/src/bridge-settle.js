// 07-T3: Pure settle predicate for Bridge two-wave code-wave panes.
//
// Problem: a code-wave pane writes "## BOUNDARIES" (the completion sentinel) BEFORE it
// runs `git commit`. The `pollBridgeReady` settle logic previously counted a pane as
// settled once `r.complete && stable` — this fired `assembleAndDispatchVerify()` while
// the `agent-teams/<id>` branch still had zero commits, so the assembler folded nothing
// and the verify wave ran against an empty integration tree.
//
// Fix: the backend now populates `r.committed` (true iff `rev-list <fork>..agent-teams/<id>`
// is non-empty). A code-wave pane is NOT settled until `r.committed` is true. Verify-wave
// panes do not commit — the predicate receives `isCodeWave=false` for them and skips the
// commit gate, preserving the existing single-wave settle behavior for those panes.
//
// Exported as a pure function so it can be unit-tested in isolation (no invoke, no DOM).

/**
 * Decide whether a single pane's `bridge_ready` row has settled.
 *
 * @param {object} r          - A row from `bridge_ready` (see PaneReady in lib.rs).
 *   r.id       {string}  - pane id
 *   r.bytes    {number}  - current byte size of the fan-in report (0 = not written)
 *   r.complete {boolean} - report contains "## BOUNDARIES" (completion sentinel)
 *   r.dead     {boolean} - PTY is gone (will never write more)
 *   r.committed {boolean} - ≥1 commit on `agent-teams/<id>` past the fork-point with HEAD
 * @param {boolean} prevBytes - byte size from the PREVIOUS poll tick (for size-stability)
 * @param {boolean} isCodeWave - true iff this pane was dispatched as a code-wave pane in
 *   a two-wave run (i.e. its id is in `bridgeCodePaneIds`). When true the commit gate is
 *   enforced; when false (verify-wave or single-wave pane) it is skipped.
 * @returns {boolean} true iff the pane has finished and its output is safe to fan-in.
 */
export function isPaneSettled(r, prevBytes, isCodeWave) {
  if (r.dead) {
    // A dead pane: settled once it has nothing (it will never write) or its partial
    // output is byte-stable (it stopped mid-write and PTY exited). The commit gate
    // is NOT applied to dead panes: they cannot run `git commit` after dying — we
    // accept whatever they left and let synthesis mark the output as partial.
    return r.bytes === 0 || (r.bytes > 0 && prevBytes === r.bytes);
  }
  const stable = r.bytes > 0 && prevBytes === r.bytes;
  // 07-T3: code-wave settle requires both the completion sentinel AND an actual commit.
  // Without the commit gate, `assembleAndDispatchVerify` fires on an empty branch and
  // the verify wave inspects a tree with no code changes (silent false-pass risk).
  if (isCodeWave) {
    return r.bytes > 0 && r.complete && stable && r.committed;
  }
  // Verify-wave and single-wave panes do not commit; complete + stable is enough.
  return r.bytes > 0 && r.complete && stable;
}
