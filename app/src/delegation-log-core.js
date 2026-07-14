// delegation-log-core — pure helpers lifted out of the Delegations / run-History surface in
// main.js. Everything here is DOM-free and global-free: time/duration formatting, the run
// timestamp/verdict/review derivations, and the stream-json → feed-entry parser. main.js keeps
// the DOM builders (renderWorkers / dgBuildDetail / dgRenderReview) and the live Maps
// (dgWorkers / dgRuns); it imports these back so the call sites are unchanged.
//
// Kept pure so the stream-json ingest (the gnarliest bit — nested message.content blocks,
// stderr prefixes, malformed lines) and the verdict/duration math can be unit-tested without
// a browser. Model-derived strings are only ever CAPPED here; the caller renders them via
// textContent (XSS-safe).

// Cap on the in-memory + rendered feed for a long run (both the accumulator and the render).
export const DG_MAX_LINES = 200;

// ts for a run: explicit record ts, else parsed from the run_id (delegate-<unix-ms>).
export function dgRunTs(id, r) {
  return (r && r.ts_ms) || parseInt(String(id).replace(/^delegate-/, ""), 10) || 0;
}

// Compact relative time ("just now" / "5m" / "3h" / "2d", else a date). For the one-line row.
export function dgRelTime(ts) {
  if (!ts) return "";
  const s = Math.max(0, Math.floor((Date.now() - ts) / 1000));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  if (s < 604800) return `${Math.floor(s / 86400)}d ago`;
  return new Date(ts).toLocaleDateString();
}

// Run DURATION = completion (ts_ms) − start (parsed from run_id "delegate-<ms>"). The full flywheel
// time (orchestrate→spawn→fix→fold→test→synth→PR) — the per-run TIMING for model-speed benchmarking.
export function dgDurationMs(runId, run) {
  // Prefer the backend's stored overall wall-clock; fall back to the legacy run_id parse for old
  // records / live headless cards (correct only for the `delegate-<ms>` id — a panes date-string
  // run_id parseInts to a tiny year → ts_ms-as-duration, the "29784739m" bug duration_ms fixes).
  if (run && Number(run.duration_ms) > 0) return Number(run.duration_ms);
  const start = parseInt(String(runId).replace(/^delegate-/, ""), 10);
  const end = run && run.ts_ms;
  return (start && end && end > start) ? (end - start) : 0;
}

export function dgFmtDur(ms) {
  if (!ms || ms < 0) return "";
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60), r = s % 60;
  return r ? `${m}m ${r}s` : `${m}m`;
}

// Plain-language verdict copy (icon pill + what it means / what to do) — NOT the raw PASS/HOLD jargon.
export function dgVerdictUx(v) {
  return {
    pass:     { pill: "✓ Verified",     mean: "A test passed and the findings were cross-checked against your code — trustworthy." },
    advisory: { pill: "✓ Report ready", mean: "An investigation run — nothing in your repo was changed. This is an advisory report; the findings aren't machine-tested, so read them as analysis to act on." },
    hold:     { pill: "⚠ Held — needs a human", mean: "Not merged. A check or a cross-examination finding needs a human decision before this can ship — open the report; the top line says exactly why (a security/scope/quality finding, or no test to confirm)." },
    reject:   { pill: "✕ Needs review", mean: "The automated check didn't pass — often a setup issue (a test failed or couldn't build), not your task itself. Open the report; the top explains why." },
    "pr-failed": { pill: "⚠ PR failed", mean: "The branch was pushed but opening the PR failed (auth/network/gh). The work is safe on the remote — open the report for the branch name, then create the PR manually." },
  }[v] || { pill: "Done", mean: "The run finished — open the report to read the findings." };
}

// P5 (unified-engine §3.6/§3.10): does THIS run carry a smart-PR-review verdict or a CRAP delta?
// Advisory/old runs carry none → null (caller skips the whole block). Pure read of the record;
// the gate-2 reviewer's APPROVE/REQUEST_CHANGES is LOGGED here but does NOT drive the verdict pill.
export function dgReviewData(run) {
  if (!run) return null;
  const dec = (run.review_decision || "").toLowerCase();
  const findings = Array.isArray(run.review_findings) ? run.review_findings : [];
  const crap = (run.crap_delta && typeof run.crap_delta === "object") ? run.crap_delta : null;
  // Nothing to show: no decision word, no findings, no crap delta, and calibration unknown.
  if (!dec && !findings.length && !crap && run.review_calibrated == null) return null;
  return { dec, findings, crap, calibrated: run.review_calibrated };
}

// Push a rendered feed entry (capped). `kind` ∈ "text" | "tool" | "result" | "raw".
export function dgPush(w, kind, text) {
  w.lines.push({ kind, text });
  if (w.lines.length > DG_MAX_LINES) w.lines.splice(0, w.lines.length - DG_MAX_LINES);
}

// Parse ONE stream-json line into zero or more feed entries appended to `w`. The blocks are
// nested: an assistant/user line carries `message.content[]` (an array) where each block has
// its own `.type` (text | tool_use | tool_result). A single line can therefore yield several
// entries. On any parse failure we show the raw trimmed line (still via textContent).
export function dgIngestLine(w, rawLine) {
  const trimmed = (rawLine || "").trim();
  if (!trimmed) return;
  // stderr lines (backend prefixes "[stderr]") → an ERROR entry so a failed worker shows WHY
  // (e.g. a transient "Service temporarily unavailable") instead of a silent "waiting…".
  if (trimmed.startsWith("[stderr]")) { dgPush(w, "error", trimmed.slice(0, 200)); return; }
  let obj;
  try { obj = JSON.parse(trimmed); } catch (_) { dgPush(w, "raw", trimmed.slice(0, 200)); return; }
  if (!obj || typeof obj !== "object") { dgPush(w, "raw", trimmed.slice(0, 200)); return; }
  const t = obj.type;
  if (t === "assistant" || t === "user") {
    const content = obj.message && Array.isArray(obj.message.content) ? obj.message.content : [];
    for (const block of content) {
      if (!block || typeof block !== "object") continue;
      if (block.type === "text" && typeof block.text === "string") {
        // cap at ingest (perf-2026-06-10 seam 2): a worker can emit multi-KB text
        // blocks; the feed is a glanceable stream, and every char ingested is re-laid-
        // out on each renderWorkers — 400 chars is plenty for the card.
        const s = block.text.trim().slice(0, 400);
        if (s) dgPush(w, "text", s);
      } else if (block.type === "tool_use") {
        const name = typeof block.name === "string" ? block.name : "tool";
        let inp = "";
        try { inp = block.input != null ? JSON.stringify(block.input) : ""; } catch (_) { inp = ""; }
        dgPush(w, "tool", `🔧 ${name}: ${inp.slice(0, 80)}`);
      } else if (block.type === "tool_result") {
        dgPush(w, "text", "↳ tool result");
      }
    }
  } else if (t === "result") {
    // terminal stream-json line: the run finished. Mark done; show a short ✓.
    if (w.status !== "retired") w.status = "done";
    dgPush(w, "result", obj.is_error ? "✗ ended with error" : "✓ done");
  } else if (t === "system") {
    // init/system lines are noise for an observer — skip (keeps the feed signal-dense).
  } else {
    dgPush(w, "raw", trimmed.slice(0, 200));
  }
}
