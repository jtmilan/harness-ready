// poll-core.js — PURE logic for the output poller + queue render diet (no DOM, no
// Tauri; vitest-covered like rail-core/bridge-chat-core). Frontend half of CONTRACT
// seam 1 (.paul/analysis/perf-2026-06-10/CONTRACT.md): main.js owns the side effects
// (invoke / term.reset / term.write / s.consumed=); everything DECIDED lives here.

// ---- poller cadence (main.js pollOutput consumes these) ----------------------
export const POLL_TICK_MS = 120;       // scheduler tick; visible panes read every tick
export const HIDDEN_EVERY = 6;         // hidden panes ride every 6th tick (~720ms) — reveal
                                       //   is made instant by an out-of-band poll on switch
export const MAX_PENDING_WRITES = 4;   // backpressure: skip a pane's reads while more than
                                       //   this many term.write callbacks are unflushed

// applyDelta — reconcile our absolute BYTE cursor against one read_output_delta reply
// {base, next, data, truncated}. Returns the decision {write, reset, consumed}:
//   - data empty                → no write; adopt `next` (cursor may still advance: the
//                                 backend holds back an incomplete trailing codepoint)
//   - truncated && consumed > 0 → reset + write: bytes we never saw were evicted from the
//                                 retained window (fell behind), or the pane respawned
//                                 under the same id (stale cursor, since > total). Either
//                                 way the terminal holds stale/garbage parser state — drop
//                                 it and repaint the retained window clean.
//   - else                      → plain write. Covers normal append (base == consumed) AND
//                                 fresh attach (consumed === 0: the terminal is EMPTY, so
//                                 the truncated retained tail needs no reset — reload/rail
//                                 click on a live pane just paints the backlog).
//   - consumed = next ALWAYS. NEVER derive offsets from JS string lengths: `next` is a
//     BYTE offset, data.length is UTF-16 code units — they differ on any multibyte output.
export function applyDelta(consumed, delta) {
  const { next, data, truncated } = delta || {};
  const adopted = typeof next === "number" ? next : consumed; // malformed reply → hold position
  // truncation outranks empty-data (review F4): a respawned-EMPTY buffer replies
  // {truncated, data:""} — skipping the reset here would let the new incarnation's
  // first output paint onto the dead incarnation's stale screen, with the reset
  // signal gone for good (the follow-up poll is in-range ⇒ truncated=false).
  if (truncated && consumed > 0) return { write: data || null, reset: true, consumed: adopted };
  if (!data) return { write: null, reset: false, consumed: adopted };
  return { write: data, reset: false, consumed: adopted };
}

// paneDue — should this pane be read on this scheduler tick? Backpressure outranks
// everything (an xterm still digesting >MAX_PENDING_WRITES chunks must not be fed —
// the backend ring retains, and the truncated-gap protocol covers a long stall).
// Visible panes read every tick; hidden ones every HIDDEN_EVERY-th (their bytes are
// consumed eventually so the backend window stays warm, but they cost ~1/6th the IPC).
export function paneDue({ visible, tick, pendingWrites }) {
  if ((pendingWrites || 0) > MAX_PENDING_WRITES) return false;
  if (visible) return true;
  return tick % HIDDEN_EVERY === 0;
}

// queueSignature — structural signature of everything pollQueue's renders are a pure
// function of (B-plan finding 3: railMeta/board columnFor have no wall-clock input).
// Same tuple → same DOM ⇒ main.js skips renderRail/renderWorkspaces/renderBoard when
// the signature is unchanged. deadPanes is sorted so Set insertion order can't make
// two identical states sign differently. The scheduler's elapsedLabel is the ONE
// time-varying surface — the caller keeps re-rendering it while `pending` is non-empty.
export function queueSignature({ queue, all, dead, activeId, activeWs, workspaces, deadPanes, pendingKeys }) {
  return JSON.stringify([
    queue, all, dead, activeId, activeWs,
    Object.keys(workspaces || {}).map((w) => {
      const ws = workspaces[w];
      return [w, ws.name, ws.color, !!ws.dormant, ws.paneIds.length, ws.count];
    }),
    [...(deadPanes || [])].sort(),
    pendingKeys || [],
  ]);
}
