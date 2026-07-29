# SYNTHESIS — harness-ready proposal session (P0→P4)

**Date:** 2026-07-29 · **Coordinator:** p0 (claude) · **Team:** ws83621x0 — p1–p4 grok builders, p5 grok scout, p6 grok verifier
**Deliverables:** `P0-ALIGNMENT.md` (audit + §6 errata) · `panes/p1.md` (16 features) · `panes/p2.md` (9 IA flows) · `panes/p3.md` (10 visual items) · `panes/p5.md` (competitive) · `panes/p6.md` (verification) · `panes/p4.md` (adversarial review)
**Rules:** R1–R10 (docs/BASE44-PROMPTS.md) · **Test gate baseline:** vitest 46/46 PASS, vite build OK — untouched (proposals only)

---

## 1 · What happened

P0 audit grounded in full read of the read-first list + contract traces (mock + Tauri surfaces). p6's independent re-audit confirmed every P0 finding, corrected one factual error (errata E1 — templates are already local-first via the offline `base44Client.js` stand-in), and added 8 misses (M1–M8). P1–P3 produced 35 schema-complete proposals. p5 mapped every area against Conductor/Superset/Claude Squad/Vibeyard/Nimbalyst/Intent with provenance tags preserved. p4's adversarial pass found 5 BLOCKING / 10 SHOULD-FIX / 6 NIT — all resolved below. Zero SKIP-list items smuggled; zero full id drops.

---

## 2 · BLOCKING resolutions (coordinator-locked — these amend the pane reports)

| # | Id | Violation | Resolution (canonical from here) |
|---|---|---|---|
| V-B1 | U-ORCH-1 / F-ORCH-1 | R3 — PRIORITY + AUTONOMY selects have no backend effect (P0 §6 M1) | **Kill-or-disable:** spawn form keeps harness + role (+ owned-paths from F-ORCH-2). PRIORITY/AUTONOMY removed, or rendered disabled with "pending backend" + NEEDS-BACKEND note. Same treatment in TemplateBuilder + recipe prefill. Folded into F-OBS-4's honesty PR as an existing-surface fix. |
| V-B2 | U-CMD-1 | R3 — registry-reconciling/stale badges without NEEDS-BACKEND | **Split:** U-CMD-1's shippable honesty work (empty count, SessionInfo, LOAD DEMO) stands; its reconciling/stale badge rows are **gated on F-GATE-UI-1 (NEEDS-BACKEND · Phase 0)** — do not ship until one liveness authority exists. |
| V-B3 | F-TPL-1 | R9/R3 — proposes removing a cloud tether that doesn't exist (errata E1) | **Retired, residual merged into F-TPL-2:** canonical R-TEMPLATES work = schema pin + JSON import/export + "Local" storage badge + import-name cleanup (`base44Client.js` silhouette causes exactly this confusion). HANDOFF.md still carries the stale "swap for a local store" line — flagged as doc debt (V-N3). |
| V-B4 | U-ONB-1 / F-ONB-1 | R3 — recipes "setting gates" over inert autonomy fields | **Reframed:** store already local; recipes carry **recommended** operator settings (documentation, not mutation) until gate mutation EXISTS; prefill restricted to EXISTS spawn fields (harness, role, repo). Coupled to F-TPL-2 DoD. |
| V-B5 | F-OBS-1 × U-CMD-2 × A-VIS-2/3 | R4 — MONITORING designed three incompatible ways; chart restyles could re-entrench vanity chrome | **IA LOCK — table-first:** Monitoring = per-pane real-signal health table (F-OBS-1) as primary surface; aggregates/charts secondary and only over real samples; every chart carries PLACEHOLDER/live source chip (A-VIS-2 scoped to chrome + chip, not hero); **SuccessRateChart / Tasks-Completed investment deferred** until task-outcome events exist on the contract (V-S5). Never P5 a chart restyle while AGENT-001…012 phantoms render. |

---

## 3 · SHOULD-FIX fold-ins (applied to canonical ids)

- **F-OBS-4 extended** with the full M1–M8 checklist (V-S8): demo button, SessionInfo, live count, "Live" copy, + PRIORITY/AUTONOMY disable (M1/V-B1), mock sharing-method parity or off-Tauri guard (M2), empty-ws cap-blame copy gated on real `atCap` (M3), ⌘⇧I/⌘G tooltip corrections (M4/M5), unrouted auth pages tagged (M6), WorkspaceTile chip props (M7), sendRaw stub doc (M8).
- **F-OBS-1 column phasing** (V-S3): ship EXISTS-signal columns first (id/kind/status/attention/branch/worktree from subscribe); metric columns land behind NEEDS-BACKEND with `no data` source chips — never zeros.
- **F-OBS-2 NOTE added** (V-S6): optional cost/token strip when a harness exposes samples (Vibeyard-parity gap from p5) — NEEDS-BACKEND, never delays process-liveness honesty, not a new pillar.
- **F-ORCH-1 + F-ORCH-2 bundled** (V-S7): role pills alone = cosmetic labeling; ship as one unit or not at all.
- **U-ORCH-2 / F-ORCH-2** (V-S4): v1 = client-side collision check + multi-`spawnAgents`; `orchestratePreview/orchestrateDispatch` stay proposed NOTEs, not EXISTS; no palette "Orchestrate" row until the handler opens a real flow.
- **U-CMD-1 keyboard** (V-S9): Alt+↑/↓ workspace cycle is **PROPOSED**, not EXISTS (code has only ⌘⇧I + ⌘G) — add handler + palette row in the same PR.
- **A-VIS §1.2** (V-S10): network series moves off `--success` to a secondary/muted stroke; sacred lanes carry state only.
- **F-MEM-1** (V-N1): selected-node ring uses `--info`/neutral, not `--need` (attention lane reserved).
- **p5 baseline** (V-S2): cloud-tether narrative deleted; templates verdict = portability hygiene (import/export), MATCH not ERODE.

---

## 4 · Canonical capability map (one id per capability for P5 picking)

| Capability | Canonical id | Design support | Status |
|---|---|---|---|
| Real-signal health table (Monitoring primary) | **F-OBS-1** | U-CMD-2, A-VIS-2/3/10 | Critical path · IA locked table-first · columns phase EXISTS-first |
| KPI source/age affordance (secondary) | **F-OBS-2** | A-VIS-3 | After F-OBS-1 · cost/token strip NOTE attached |
| Real-samples series chart (optional) | F-OBS-3 | A-VIS-2 | Only over real samples |
| Micro-honesty PR (M1–M8) | **F-OBS-4** | A-VIS-7 | Shippable now · EXISTS-only · strongest trust PR |
| Local templates DoD (schema pin + import/export + badge) | **F-TPL-2** | U-ONB-1 store notes | Independent · absorbs retired F-TPL-1 residual |
| CONTEXT memory-graph surface (read-only) | **F-MEM-1** | U-CMD-3 | NEEDS-BACKEND read API · BORROW |
| Memory→worktree jump (advisory) | F-MEM-2 | — | After F-MEM-1 |
| Role-cast + owned-paths + collision WARNING (bundle) | **F-ORCH-1 + F-ORCH-2** | U-ORCH-1/2 | ADAPTER · advisory-only · client-side v1 |
| Recipes/Playbooks manager | **F-ONB-1** | U-ONB-1 (reframed) | Independent parallel · recommended-settings-only |
| First-run recipe from empty state | F-ONB-2 | — | After F-ONB-1 |
| Command palette over existing handlers | **F-PAL-1** | U-PAL-1 | Opportunistic · handler-first rule |
| Skills catalog index | F-SKL-1 | — | Opportunistic · thin |
| Registry reconciling badge | F-GATE-UI-1 | U-CMD-1 badge rows | **NEEDS-BACKEND · Phase 0 · do not ship** |
| Shared-state layer indicator | F-MCP-UI-1 | — | **NEEDS-BACKEND · Phase 7 · do not ship** |
| Ranked attention strip + queue semantics | U-QUE-1 | — | EXISTS-backed · HANDOFF score divergence noted (V-N4) |
| Per-pane inline reply + RESUME-always | U-QUE-2 | — | EXISTS-backed · already largely live |
| Lane token pack + dark-default :root | **A-VIS-1** | — | Zero-backend prerequisite for all chrome |
| Ambient token layer (scanline/grid) | A-VIS-4 | — | Extends existing .scanlines |
| Motion tokens + reduced-motion kill switch | A-VIS-5 | — | |
| Focus-visible ring system | **A-VIS-6** | — | R8 prerequisite |
| Type scale utilities (tabular/small-caps) | A-VIS-8 | — | |
| data-skin variable slots (no switcher) | A-VIS-9 | — | Structure only — no theme gallery (R2) |

---

## 5 · Sequencing (BUILD-PLAN §1 aligned, p4-verified PASS)

1. **Critical path:** Phase 0 R-GATES (HD, backend) → **F-OBS-1** table (1a) → attribution bars (1b, needs Phase 0 liveness).
2. **Trust PR now (parallel, zero backend):** **F-OBS-4** + **A-VIS-1** + **A-VIS-6** — kills every fake/simulated surface P0/p6 found and lays lane/focus tokens under everything else.
3. **Must-haves next:** **F-TPL-2** ∥ **F-MEM-1** (when read API lands) ∥ **F-ORCH-1+2 bundle**.
4. **Opportunistic:** **F-ONB-1** (parallel track) · **F-PAL-1** (after handlers stable) · F-SKL-1.
5. **Gated/last:** F-GATE-UI-1 (Phase 0 design) · F-MCP-UI-1 (Phase 7 HD).

---

## 6 · Operator pick-list for P5

Ranked candidates (p4 Top 5, coordinator-endorsed):

| Pick | Id | Why | Effort |
|---|---|---|---|
| 1 | **F-OBS-4** | Pure honesty on EXISTS paths — kills the dead Demo CTA, fake SessionInfo, "Live" copy, inert PRIORITY/AUTONOMY knobs (M1–M8). Instantly makes the app stop lying. Rides with any later Monitoring PR. | S |
| 2 | **F-OBS-1** | The differentiator: per-pane real-signal table with honest empty/stale/state_blind states. Pick after F-OBS-4; metric columns phase behind NEEDS-BACKEND. | M |
| 3 | **A-VIS-1 + A-VIS-6** | Sacred lane tokens + dark-default `:root` + focus rings. Zero backend; prerequisite for honest chart/status chrome everywhere. | S–M |
| 4 | **F-ORCH-1 + F-ORCH-2** (bundle) | Advisory role-cast + owned-paths + amber collision WARNING — distinctive ADAPTER nobody else ships with this honesty (warn-not-block). | M |
| 5 | **F-TPL-2** | Import/export + schema pin + Local badge — portability hygiene, fully local-first already. | S |

**Do NOT pick as written:** F-TPL-1 (retired → F-TPL-2), U-ORCH-1/U-ONB-1 (pre-amendment versions — use §2/§3 resolutions), any SuccessRateChart-only restyle (V-B5), F-GATE-UI-1 / F-MCP-UI-1 (backend-gated).

**To start P5:** run BASE44-PROMPTS.md P5 with `id = F-OBS-4` (or your pick) against the local checkout; the canonical resolutions in §2–§3 are binding context.

---

## 7 · Coordinator self-review footer (P4 checklist, session-level)

[✅] R1 ICP — zero beginner-only items survived; recipes ≠ courses. [✅] R2 SKIP — 0 regressions (p4 §6); A-VIS-9 ships no gallery. [⚠️→✅] R3 — 3 BLOCKING fails (PRIORITY/AUTONOMY, registry badges, recipe gates) resolved via §2; M1–M8 folded into F-OBS-4. [⚠️→✅] R4 — IA split locked table-first; chart investment deferred until real signals. [✅] R5 — lanes on dedicated tokens (A-VIS-1); network series moved off `--success`. [⚠️→✅] R6 — false cloud-tether narrative deleted (E1); no meters/upsell anywhere. [✅] R7 — dual-path discipline; Alt+↑/↓ reclassified PROPOSED. [✅] R8 — focus rings, color+label, reduced-motion in spec. [✅] R9 — no invented BridgeMind surfaces; watchlist honored. [✅] R10 — contract untouched; new needs are NOTEs only.
**Dropped:** 0 items · 1 id retired into another (F-TPL-1→F-TPL-2) · 2 re-scopes · 1 demote-if-alone rule. **Audit trail:** p6 errata applied to P0 §6; p4 citations spot-checked by coordinator (spawn_workspace args, base44Client stand-in both verified against source).

---

*Files: P0-ALIGNMENT.md · panes/p1.md · panes/p2.md · panes/p3.md · panes/p4.md · panes/p5.md · panes/p6.md · panes/tasks/*.md (pane briefs). All uncommitted — review, then branch.*
