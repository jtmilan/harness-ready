// Tests for the PURE poller core (poll-core.js, perf-2026-06-10 seam 1). Protocol
// decisions only; the side effects (invoke/term.reset/term.write) are main.js glue
// and are GUI-verified, not unit-tested here.
import { describe, expect, it } from "vitest";
import {
  applyDelta, paneDue, queueSignature,
  HIDDEN_EVERY, MAX_PENDING_WRITES,
} from "./poll-core.js";

describe("applyDelta", () => {
  it("fresh attach (consumed=0) to a live pane with a truncated retained tail → write, NO reset", () => {
    // reload / rail-click on a long-running pane: since=0 fell below the buffer base,
    // backend returns the retained window truncated=true. The terminal is empty —
    // resetting would be pointless (and would flash); just paint the backlog.
    const r = applyDelta(0, { base: 4_000_000, next: 4_200_000, data: "tail", truncated: true });
    expect(r).toEqual({ write: "tail", reset: false, consumed: 4_200_000 });
  });

  it("truncated mid-stream (fell behind the retained window) → reset THEN write", () => {
    const r = applyDelta(1024, { base: 8192, next: 9000, data: "gap-tail", truncated: true });
    expect(r.reset).toBe(true);
    expect(r.write).toBe("gap-tail");
    expect(r.consumed).toBe(9000);
  });

  it("stale cursor (backend pane respawned under the same id) → reset + adopt the new base", () => {
    // since(9999) > total: backend returns the whole (new) window truncated=true with
    // next far BELOW our cursor. Protocol-wise identical to the gap case — reset+write
    // — and consumed must adopt `next`, never keep the dead incarnation's offset.
    const r = applyDelta(9999, { base: 0, next: 300, data: "fresh-boot", truncated: true });
    expect(r).toEqual({ write: "fresh-boot", reset: true, consumed: 300 });
  });

  it("normal append (base == consumed, not truncated) → plain write, cursor advances to next", () => {
    const r = applyDelta(100, { base: 100, next: 150, data: "x".repeat(50), truncated: false });
    expect(r.reset).toBe(false);
    expect(r.write).toBe("x".repeat(50));
    expect(r.consumed).toBe(150);
  });

  it("empty data → no write, no reset, consumed = next ALWAYS (held-back codepoint can move it)", () => {
    expect(applyDelta(100, { base: 100, next: 100, data: "", truncated: false }))
      .toEqual({ write: null, reset: false, consumed: 100 });
    // truncated + empty + mid-stream cursor: RESET anyway (review F4) — this is the
    // respawned-empty-buffer shape; skipping the reset would strand the dead
    // incarnation's screen (the follow-up poll is in-range ⇒ truncated=false).
    expect(applyDelta(100, { base: 0, next: 0, data: "", truncated: true }))
      .toEqual({ write: null, reset: true, consumed: 0 });
    // truncated + empty on a FRESH cursor: terminal is already empty — no reset.
    expect(applyDelta(0, { base: 0, next: 0, data: "", truncated: true }))
      .toEqual({ write: null, reset: false, consumed: 0 });
  });

  it("NEVER derives the cursor from JS string lengths: multibyte data, consumed = backend next", () => {
    // "✓✓" = 2 UTF-16 units but 6 UTF-8 bytes; the backend's `next` is byte-absolute.
    const r = applyDelta(10, { base: 10, next: 16, data: "✓✓", truncated: false });
    expect(r.consumed).toBe(16);          // base + 6 bytes — from `next`,
    expect(r.consumed).not.toBe(10 + 2);  // never base + data.length
  });

  it("malformed reply (no numeric next) holds position instead of corrupting the cursor", () => {
    expect(applyDelta(42, {}).consumed).toBe(42);
    expect(applyDelta(42, null).consumed).toBe(42);
  });
});

describe("paneDue", () => {
  it("visible pane: due every tick", () => {
    for (let t = 1; t <= HIDDEN_EVERY + 1; t++) {
      expect(paneDue({ visible: true, tick: t, pendingWrites: 0 })).toBe(true);
    }
  });

  it("hidden pane: due only every HIDDEN_EVERY-th tick (~750ms lane)", () => {
    for (let t = 1; t <= 2 * HIDDEN_EVERY; t++) {
      expect(paneDue({ visible: false, tick: t, pendingWrites: 0 }))
        .toBe(t % HIDDEN_EVERY === 0);
    }
  });

  it("backpressure: > MAX_PENDING_WRITES unflushed term.write callbacks → never due, even visible", () => {
    expect(paneDue({ visible: true, tick: HIDDEN_EVERY, pendingWrites: MAX_PENDING_WRITES + 1 })).toBe(false);
    // exactly AT the high-water mark is still allowed (skip only while strictly above)
    expect(paneDue({ visible: true, tick: 1, pendingWrites: MAX_PENDING_WRITES })).toBe(true);
  });
});

describe("queueSignature", () => {
  const base = () => ({
    queue: [{ id: "ws1-p0", harness: "claude", state: "working", reason: "-", needs_human: false }],
    all: ["ws1-p0"],
    dead: [],
    activeId: "ws1-p0",
    activeWs: "ws1",
    workspaces: { ws1: { name: "alpha", color: "#fff", dormant: false, paneIds: ["ws1-p0"], count: 1 } },
    deadPanes: new Set(),
    pendingKeys: [],
  });

  it("same structural inputs → same signature (the skip is sound)", () => {
    expect(queueSignature(base())).toBe(queueSignature(base()));
  });

  it("each render-relevant field perturbation changes the signature", () => {
    const sig = queueSignature(base());
    const perturb = [
      (a) => { a.queue[0].needs_human = true; },             // rail mark / ws amber dot
      (a) => { a.queue[0].state = "error"; },                // board column + error dot
      (a) => { a.dead = ["ws1-p0"]; },                       // dead sweep result
      (a) => { a.activeId = null; },                         // rail .active row
      (a) => { a.activeWs = "ws2"; },                        // ws .active row
      (a) => { a.workspaces.ws1.dormant = true; },           // dormant tier
      (a) => { a.workspaces.ws1.paneIds = []; },             // pane count badge
      (a) => { a.deadPanes = new Set(["ws1-p0"]); },         // red dot tier
      (a) => { a.pendingKeys = ["ws1-p9"]; },                // scheduler rows
    ];
    for (const p of perturb) {
      const args = base();
      p(args);
      expect(queueSignature(args)).not.toBe(sig);
    }
  });

  it("deadPanes Set insertion order does not change the signature (sorted)", () => {
    const a = base(); a.deadPanes = new Set(["b", "a"]);
    const b = base(); b.deadPanes = new Set(["a", "b"]);
    expect(queueSignature(a)).toBe(queueSignature(b));
  });
});
