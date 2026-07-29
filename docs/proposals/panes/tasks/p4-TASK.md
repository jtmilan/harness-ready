# TASK p4 — ADVERSARIAL P4 REVIEW (harness-ready proposals)  [WAVE 2 — runs after p1/p2/p3/p5 land]

You are builder pane p4 in workspace ws83621x0, acting as the hostile reviewer. Coordinator is p0. Deliverable is a REVIEW REPORT — no application code.

## Required reading (all of it, in full)
1. /Users/jeffrymilan/Personal/harness-ready/docs/BASE44-PROMPTS.md — R1–R10 + P4 self-review checklist (this is your weapon)
2. /Users/jeffrymilan/Personal/harness-ready/docs/proposals/panes/p1.md — P1 feature proposals
3. /Users/jeffrymilan/Personal/harness-ready/docs/proposals/panes/p2.md — P2 UI/IA proposals
4. /Users/jeffrymilan/Personal/harness-ready/docs/proposals/panes/p3.md — P3 visual spec
5. /Users/jeffrymilan/Personal/harness-ready/docs/proposals/panes/p5.md — competitive scout note
6. /Users/jeffrymilan/Personal/harness-ready/docs/proposals/P0-ALIGNMENT.md — ground truth (contract coverage table §2 especially)
7. /Users/jeffrymilan/Personal/harness-ready/ui/HANDOFF.md + ui/src/lib/agentBridge.js when a backend_dep claim needs checking

## Mission
Hunt violations in EVERY proposal item, one by one:
- **R1 ICP**: beginner-only items disguised as power-user features? Name them.
- **R2 SKIP list**: drag-kanban / voice / beginner video / vendor benchmark / theme gallery / three modes / cloud-first smuggled in under other names? (Watch for "mission control drag board", "voice notes", "getting started tour", "skin gallery".)
- **R3 fake affordances**: any `backend_dep: EXISTS` that is NOT actually on the contract (check P0 §2 + agentBridge.js)? Any control without a named method/state not tagged NEEDS-BACKEND?
- **R4 monitoring honesty**: any proposal that reintroduces vanity KPIs, interpolation, or simulated data without placeholder affordance? Missing stale/state_blind states?
- **R5 lanes**: state colors wearing brand? Charts using non-lane colors? Hardcoded hex sneaking into the spec?
- **R6**: credit/usage/upsell/cloud-account framing anywhere?
- **R7**: palette-only shortcuts? Actions unreachable by key? Missing typing-target guards?
- **R8**: color-as-sole-signal? Missing focus rings? Motion without reduced-motion off-switch?
- **R9**: invented BridgeMind REST/webhooks/CLI/fleet-dashboard treated as real?
- **R10**: backend contract redefined instead of proposed as a NOTE?
- **Cross-consistency**: p1/p2/p3 proposals that contradict each other (same surface designed twice differently), duplicate ids, sequencing notes that violate BUILD-PLAN §1 ordering.
- **Competitive sanity** (vs p5): any proposal the scout flagged as ERODE/drop that survived unmodified?

## Report format
Per violating item: `id · violation (which R-*) · evidence (quote the proposal line) · fix (drop / rewrite how / tag NEEDS-BACKEND)`. Severity: BLOCKING (must fix before operator picks an id) / SHOULD-FIX / NIT.
Then: **top 5 strongest proposals** (what survived your attack cleanly — the operator's best P5 candidates, with one-line justification each) and a **verdict footer**: total items reviewed, blocking count, should-fix count, drops recommended.

## Rules
- Be adversarial but fair: cite the exact proposal text for every violation. No vibe-based objections.
- Do not rewrite proposals — flag + prescribe. The coordinator synthesizes.

## Output
Write the full report to /Users/jeffrymilan/Personal/harness-ready/docs/proposals/panes/p4.md (coordinator's checkout, NOT your worktree). End with `## BOUNDARIES`. Then report completion.
