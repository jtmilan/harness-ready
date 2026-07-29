# Research Synthesis — BridgeMind × harness-ready (consensus report)

**Status:** Final synthesis · **Date:** 2026-07-29 · **For:** the `harness-ready` product (`/Users/jeffrymilan/Personal/harness-ready`).
**Companion docs:** `PRD.md` (requirements), `DESIGN-BRIEF.md` (UX/visual), `BUILD-PLAN.md` (phased build). This file is the **one-page-to-the-bottom consensus** those three hang off.

> **Method & provenance (read first).** Four deep-dive lenses (product-surface, orchestration/architecture, commercial/moat, ecosystem/benchmark) ran in parallel, each assigned a polyglot-broker closed-enum domain posture (`architecture` / `ai_ml` / `general` / `testing`); a fifth **adversarial skeptic** (`security` posture) then stress-tested every load-bearing claim and reconciled cross-lens contradictions. The BridgeMind live site is **Cloudflare-blocked** (WebFetch 403, curl "Just a moment"), so **no finding is extracted from the live DOM** — all BridgeMind detail is from indexed sub-pages, the official `/pricing` snippets, the open-source GitHub org, and third-party reviews (chiefly starsearn.com), tagged **EXTRACTED / INFERRED / AMBIGUOUS**. Community/ecosystem articles that merely *name* BridgeMind as one example (e.g. pubroot coordination write-ups) were **not** treated as BridgeMind-internal fact.
> **Harness cross-LLM vote: NOT in this synthesis.** A live `team_prompt_all` to the running fleet (`ws50611x0`) proved the send + coordinator gates are open, but the grok TUIs staged the long prompt in a bracketed-paste "press Enter to submit" overlay and did not auto-answer, so **no grok/codex/cursor model vote is included here** — including one would be fabrication. That pass remains available on one word (press Enter in the panes + `read`, or `yes headless`); when captured it appends to this report, it does not silently replace it. The lenses themselves were in-process subagents (one model family), so treat the "consensus" below as *multi-lens + adversarial* consensus, **not** multi-LLM consensus.

---

## 0. TL;DR — the consensus verdicts (what every lens + the skeptic agree on)

1. **BridgeMind is a desktop-native ADE ("BridgeSpace"), not a cloud service** [EXTRACTED; "cloud-first" was REFUTED by the skeptic]. It bundles multi-agent coding (BridgeSwarm), persistent memory (BridgeMemory), an MCP server (BridgeMCP), voice (BridgeVoice), a two-way-sync kanban (BridgeBoard), and an OSS benchmark (BridgeBench) into one window, sold as credit SaaS ($16–$100/mo, **no free tier**) to **non-technical "vibe-coding" MVP builders** [EXTRACTED: "Cursor recommended for experienced developers"].
2. **harness-ready serves a different ICP** — terminal-native power users who *orchestrate* coding agents. **This ICP mismatch is the working hypothesis that governs every decision below** (held with a 12-month kill-switch — see point 7). Chasing BridgeMind feature-for-feature would import beginner-ICP dead weight into a power-user tool.
3. **Therefore: do not pursue parity.** **BORROW** the persistent-memory idea + the open-source-skills *posture*; **ADAPTER** the role-cast orchestration *pattern* + multi-pane UX; **MAKE REAL** fleet monitoring (a harness-ready **differentiator** — BridgeMind has *none*); **SKIP** drag-kanban, voice, tutorial hub, vendor benchmark, themes, "three modes", cloud-first.
4. **BridgeMind's moat is narrow and media-weighted** (founder build-in-public distribution + per-user memory graphs). **Nuance (cross-LLM review, applied 2026-07-29, item 1):** MCP being open by design weakens the *protocol/integration* moat only — openness ≠ weak *execution* moat (OCI is open; Docker/K8s stay moated). So do not copy their defensibility; compete on technical differentiation — and note **harness-ready's local-first execution is itself a real moat**, not just a feature list.
5. **Several widely-circulated "BridgeMind mechanics" are not BridgeMind facts** — container sandbox, automatic worktree-per-agent, the SWARM_BOARD/bs-mail/mutex/BCL coordination internals, and "three modes" are REFUTED or INFERRED-only. **Do not build toward them.**
6. **harness-ready already wins the axes its ICP cares about** — automatic git-worktree-per-agent, ranked attention queue, autonomy gates, 8-harness breadth incl. terminal-native CLIs, local-first/BYO-key. **Protect and amplify these; do not dilute them.**
7. **Confidence & kill-switch (cross-LLM review, applied 2026-07-29, item 2):** independent external-model adversarial votes (Anthropic 62, xAI 42 / 100) put confidence in this direction at **42–62/100** and named **ICP overlap over ~12 months as the single hinge**. Hold the spine as monitored; if BridgeMind moves upmarket or operators cross over, re-run the §3 SKIP list.

---

## 1. BridgeMind — the corrected product picture

| Surface | What it is | Evidence | Synthesis stance |
|---|---|---|---|
| BridgeSpace | Desktop ADE: multi-pane terminals (1–16) + code editor + BridgeBoard + mission tree | EXTRACTED | The real product shell; **desktop-native** |
| BridgeSwarm | Orchestrator + workers: closed roles **Coordinator / Builder / Scout / Reviewer**; per-role model; "up to 16 agents"; file-ownership | EXTRACTED (roles/marketing) | Pattern worth *adapting*; the detailed coordination mechanics are **INFERRED** (community articles), not internals |
| BridgeMemory | Per-project `.bridgememory/` markdown graph, MCP-native, compounds across sessions; **Pro+ gate** | EXTRACTED | The *value* (compounding context) is worth borrowing; the paid gate is not (we're local/BYOK) |
| BridgeMCP | MCP server = persistent kanban + shared context + `taskKnowledge`; editor clients connect as MCP clients | EXTRACTED | Their "API story" **is** MCP + keys + OSS skills — there is **no REST/webhooks/CLI** to copy; read-vs-write gating AMBIGUOUS |
| BridgeVoice | Push-to-talk dictation, 99+ langs, ~150 WPM, system-wide | EXTRACTED | SKIP — orthogonal to terminal-native ICP |
| BridgeBoard | Two-way-sync kanban; drag-dispatch; review column | EXTRACTED | SKIP as a *dispatch surface* (power users dispatch via CLI/orchestrate/task-list) |
| BridgeBench | OSS (MIT), 130+ tasks, blind 3-judge + Elo, speed+cost+quality | EXTRACTED (exists); credibility AMBIGUOUS (task provenance / judge pinning / independent repro unknown) | SKIP a *vendor* benchmark; reference SWE-bench/ViBench |
| Settings (evidenced) | API Keys (named); theme picker (Paper/Chalk/Solar/Arctic/Ivory); BridgeVoice widget; credit balance | EXTRACTED | Note gaps vs us: no trusted-repos allowlist, no autonomy gates, **no fleet telemetry** |
| Settings/API (NOT evidenced) | REST API, webhooks, standalone CLI, per-session memory toggles, shortcut customization | AMBIGUOUS/absent | Do not assume they exist |

**Cross-lens reconciliation applied here:** D2 had called "container sandbox per worktree" *confirmed* and "git-worktree-per-agent" *confirmed*; D1 had worktree as *AMBIGUOUS* and D4 had called the platform *cloud-first*. The skeptic **REFUTED** the sandbox (no primary source) and **cloud-first** (it's desktop-native), and **harmonized** worktree to: *"BridgeSpace 3 ships worktree **UI controls** (changelog v3.0.84+); automatic worktree-per-agent isolation is NOT evidenced."* Net: **our automatic worktree-per-agent is a differentiator, not a gap.**

---

## 2. Competitive position & honest moat

- **Real competitive set = Lovable / Bolt.new / Replit Agent** (non-technical MVP builders), **not** Cursor / Cline / Aider. Reviews cluster BridgeMind with the "vibe-coding" cohort; the product explicitly defers experienced devs to Cursor. [EXTRACTED/INFERRED.]
- **Pricing:** Basic $20/$16-annual · 5k credits; Pro $50/$40 · 12.5k + memory/MCP/voice; Ultra $100/$80 · 25k. **No free tier; 7-day money-back; per-action credit schedule undisclosed.** [EXTRACTED: /pricing + starsearn; unit economics AMBIGUOUS.] The opaque credits + multi-agent cost-compounding is a documented market backlash vector — harness-ready's BYO-key/local model **sidesteps it entirely**; keep it that way.
- **Moat (not sandbagged):** durable = founder-led media distribution (YT ~97k, TikTok, X ~24k, Discord ~7–8k; ARR ~$245–250k around day ~209 — **self-reported, un-audited**) + per-user memory graphs. **Weak at the *protocol* layer** = technology (orchestrator over third-party LLMs), integrations (replicable), and protocol lock-in (MCP is open *by design*). **Correction (cross-LLM review, applied 2026-07-29, item 1):** openness removes *protocol* lock-in, not *execution* moat (OCI open; Docker/K8s moated) — so BridgeMind's execution moat is real-if-unproven, and **harness-ready's local-first execution is a genuine moat**. [EXTRACTED/INFERRED; correction INFERRED.] **Takeaway:** do not invest in defensibility-by-feature or a media strategy; compete on observability + autonomy gates + harness-agnostic orchestration + local-first execution-as-moat.
- **Market context:** agentic AI ~$9.9B (2026), ~42% CAGR, but only ~10–14% of pilots reach production; the consensus architecture is *orchestrator + isolated workers* (not peer swarms), *MCP + A2A* protocols, *kanban-as-control-plane*, and *observability as the reliability + monetization layer*. harness-ready already embodies most of these. [EXTRACTED: vault `orchestration-market-landscape-2026`.]

---

## 3. The ICP-mismatch law → the transfer matrix (the actionable core)

Test applied to every BridgeMind feature: *does this serve a terminal-native power user, or only a beginner?*

| BridgeMind capability | Evidence | Our ICP fit | Decision | One-line why |
|---|---|---|---|---|
| Persistent context / memory graph | EXTRACTED | Yes | **BORROW** | We have the backend (`memory` crate + MCP tools); add an always-available graph view; never tier-gate it |
| Open-source skills *posture* | EXTRACTED | Yes | **BORROW** | Validates publishing our `.claude/skills`; we already have the catalog |
| Role-cast orchestration (Coord/Builder/Scout/Reviewer) + file-ownership | EXTRACTED (roles) | Yes (pattern) | **ADAPTER** | Take role-casting + ownership *convention*; keep our harness-agnostic `team_orchestrate`; we already auto-isolate via worktrees |
| Multi-pane terminals | EXTRACTED | Yes | **ADAPTER** | Our PTY-level grid already exceeds their UI-level panes — no gap |
| **Fleet telemetry / monitoring** | **REFUTED for them** | Yes | **MAKE REAL (ours)** | They have none; our simulated MONITORING tab → real = a **differentiator** |
| Command palette | AMBIGUOUS | Marginal | **OPTIONAL (P2)** | Convenience, not core; power users live in shells |
| Drag-dispatch kanban (BridgeBoard) | EXTRACTED | No | **SKIP** | Beginner affordance; our task lifecycle + orchestrate suffice |
| Voice dictation (BridgeVoice) | EXTRACTED | No | **SKIP** | Orthogonal to terminal workflow |
| Learn / content hub | EXTRACTED | Partial | **SKIP** beginner courses / **ADD** onboarding recipes (applied 2026-07-29, item 4) | Beginner *video/course* content = dead weight for this ICP; but 8 harnesses + autonomy gates make **activation** a power-user problem → ship **recipes/playbooks** (per-harness autonomy profiles), not courses (R-ONBOARD) |
| Vendor benchmark (BridgeBench) | EXTRACTED; credibility AMBIGUOUS | No | **SKIP** | Vendor self-dealing + unverified reproducibility; reference SWE-bench/ViBench |
| App themes | EXTRACTED | No | **SKIP** | Power users theme via shell/editor |
| "Three modes" / cloud-first / auto-worktree / container sandbox | NOT evidenced / REFUTED | n/a | **SKIP (mirage)** | See watchlist (§6) |

---

## 4. harness-ready differentiators to protect & amplify

Automatic git-worktree-per-agent · ranked who-needs-you attention queue · **real** fleet telemetry (once wired) · fine-grained autonomy gates (`allow_mutations`, `send_input_enabled`, `autonomy_ceiling`, `external_spawn_max_panes`) · `broadcast` / `delegate` primitives · append-only audit log · 8-harness breadth incl. terminal-native CLIs (opencode/commandcode/pi/grok/bash) that BridgeMind ignores · local-first / BYO-key (no credit trap, no vendor cloud lock-in). **Guard metric for every future PR: count of beginner-ICP features shipped = 0.**

---

## 5. Prioritized recommendations (what to build, in order)

| # | Recommendation | Type | Priority | Effort | Human-design stop? |
|---|---|---|---|---|---|
| R-OBS | Make the MONITORING tab **real** (replace `monitorData.js`/`MockAgentBridge` with per-process samples + real task-outcome events; honest stale/`state_blind` states) | MAKE REAL / differentiator | **P0** | M | No |
| R-GATES | Close **liveness-blindness** + **branch-wire-through** (single source of liveness; read/mutation parity) | trust-critical | **P0** | L | **YES** |
| R-TEMPLATES | Move templates off Base44 → local store (keep `AgentTemplate` schema; add JSON import/export) | close gap | P1 | S–M | No |
| R-MEM | Compounding persistent context + read-only graph view (the BridgeMemory *value*, local-first, never tier-gated) | BORROW | P1 | M | **YES if** the memory write-contract changes |
| R-ORCH | Role-cast orchestration + file-ownership **on the task channel only** (never the persona SSOT) + collision *warning* in the orchestrate preview | ADAPTER | P1 | M | No if channel-B only; **YES** if it touches persona SSOT |
| R-PALETTE | `Cmd/Ctrl+K` palette over *existing* handlers (no palette-only shortcuts) | optional | P2 | S–M | No |
| R-SKILLS-OSS | Publish the skills catalog (landing/index; MIT where appropriate) | BORROW | P2 | S | No |
| R-MCP-RW | Optional BridgeMCP-*like* read-write shared-state layer, gated-OFF by default | optional | P2 | L | **YES** (crosses the mutation boundary) |

Sequencing & gates: see `BUILD-PLAN.md`. Critical path = R-GATES → R-OBS attribution. **Test gate is read from the repo and never redefined**; one slice per PR; human deep-review; never auto-merge; commit-on-ask.

---

## 6. Vaporware / mirage watchlist — do NOT build toward these as "parity"

1. Cloud-first architecture — **REFUTED** (BridgeSpace is desktop-native).
2. Automatic git-worktree-per-agent — **overstated** (they ship worktree *UI* in B3; we already have the real thing).
3. Container sandbox per worktree — **no source** (was inferred from `/sandbox` norms).
4. "Three modes (vibe/agentic/manual)" — **not evidenced**.
5. A BridgeMind-side fleet KPI dashboard — **does not exist** (hence not a gap for us; it is the opposite).
6. The SWARM_BOARD 6-col schema / `bs-mail` 30–60s pull / `CLAIM·WAIT·RELEASE` mutex / BCL / 3-coordinator mechanics — **INFERRED-only** (community articles), not BridgeMind internals.

---

## 7. Open questions & gaps (carry as AMBIGUOUS; verify before depending on them)

- BridgeMind: command-palette scope; explicit approve/reject HITL UX; BridgeMCP read-vs-write gating; "25+ tools" enumeration; **real credit burn-rate**; **failure/retry/rollback model**; harness diversity beyond marketing; OS support for BridgeSpace; multi-user/real-time collab; enterprise features (SSO/audit/SOC2 — not found).
- BridgeBench: canonical reproduction command; judge-model pinning; task provenance; any independent reproduction.
- Analysis gaps the skeptic flagged: BridgeMCP tool-surface detail; BridgeBench coverage vs infra/db/security workloads.
- **Verify via a real browser / Google cache** before coding any feature that hinges on an AMBIGUOUS BridgeMind detail (the live site stays unreachable to fetch tools).

---

## 8. What the consensus says to *stop* doing / avoid

- Stop framing monitoring as catch-up — it is a lead.
- Stop treating git-worktree isolation as a gap — it is a lead.
- Stop importing drag-kanban / voice / tutorial onboarding / themes — they serve the wrong ICP and dilute the power-user surface.
- Stop trusting marketing mechanics as specs (sandbox, auto-worktree, three modes, the pubroot coordination internals).
- Never ship a control without its backing command/state (no fake affordances — e.g. no SKIP-permissions toggle without a backend flag; RESUME stays one-way because no `paused` state is recorded).
- Never present simulated data without a visible "placeholder" affordance (the current MONITORING state, until R-OBS).

---

## 9. Provenance & method appendix

- **Lenses:** D1 product-surface (tag `architecture`), D2 orchestration/arch (`ai_ml`), D3 commercial/moat (`general` — the closed enum has no market tag; documented deviation), D4 ecosystem/benchmark (`testing`); skeptic (`security`). Domain postures read verbatim from the polyglot-broker catalog; no live fetch.
- **Cross-lens contradictions resolved by the skeptic:** sandbox CONFIRMED→REFUTED; cloud-first→REFUTED (desktop-native); worktree CONFIRMED/AMBIGUOUS→harmonized (UI controls only); monitoring "gap"→REFRAMED (our differentiator); pubroot mechanics EXTRACTED→INFERRED; credit stress-test retained only as a *labelled proxy* (Augment Code math, not BridgeMind's); moat read confirmed as honest (not sandbagged).
- **What is NOT in this synthesis:** a multi-LLM harness vote. The live `team_prompt_all` to `ws50611x0` returned `errors:[]` (send_input_enabled ON, coordinator gate passed) but each grok TUI staged the prompt in a bracketed-paste review overlay and did not auto-submit; capturing an answer requires a real `Enter`/`Esc` at the TUI or a headless run. **No harness-model output is represented above.** When obtained, it is appended as §10, not merged silently.
- **Skills note:** `/polyglot-broker` and `/graphify` *skills* were not installed in this environment; the broker's closed 11-tag enum was applied manually (catalog verbatim) and graphify via its CLI.

---

## 10. Cross-LLM adversarial votes (headless harness pass)

**Method.** Each harness binary was run one-shot headless in `/tmp` with the same adversarial prompt (read-nothing, no tools, no writes), capturing stdout. This is the *real cross-LLM* layer the in-process qwen lenses could not provide (those share one model). **Panel captured: 2 of 4 external families** — `claude` (Anthropic) and `grok` (xAI) returned clean votes; `codex` (OpenAI) hung waiting on stdin and `opencode` produced no output (non-tty / wrong subcommand) — a bounded retry for those two runs in the background and, if it lands, is appended under this section without renumbering. The qwen 4-lens + skeptic analysis above is the *baseline*, not a vote in this panel.

### Claude (Anthropic) — confidence 62/100
- **Q1 (steelman parity):** weak; rejects it. The funnel-migration steelman rests on inference, not data; BridgeMind's own "Cursor for experienced devs" line cuts against it.
- **Q2 (dangerous assumption):** *ICPs stay separate* — the whole spine hinges on it. **Plus a logic bug in our moat claim:** "MCP open by design ⇒ integration moat weak" is **false**; an open protocol does not erase the implementation moat (OCI is open; Docker/K8s stay moated). Openness kills *protocol* lock-in, not *execution* moat.
- **Q3 (flip):** SKIP learn-hub → **BORROW minimal onboarding** ("recipes/playbooks": one-click autonomy profiles per harness). 8 harnesses + gates = a brutal activation curve; onboarding is existential, not optional.
- **Q4 (under-weighted):** **remote / multi-machine fleet** — a supervisory plane over SSH/daemon workers reporting into the queue. Nobody serious runs 16 agents on one MacBook; this is where "Command Center" earns its name, and it wins teams too.
- **Perspective:** over-weights distribution/network moats, under-weights "just build a better local tool"; discount its Q4 ~15%.

### Grok (xAI) — confidence 42/100
- **Q1 (steelman parity):** memory + orchestration + attention + fleet-visibility parity may be needed just to enter the same buying conversation; but evidence is **unverified** — treat as hypotheses.
- **Q2 (dangerous assumption):** same hinge (ICP overlap), plus: misreading BridgeMind *positioning* as *mechanics* could make us under-build the multi-agent coordination layer that actually differentiates both products.
- **Q3 (flip):** **promote** role-cast + a minimal durable work/attention board to **first-class**; **demote** fleet monitoring until it carries real signals (a simulated tab teaches distrust). Do **not** SKIP the *ownership / claim-wait semantics* by association with the skipped kanban chrome.
- **Q4 (under-weighted):** **productize** an exportable, git-friendly, open-format agent-shared memory+skill surface (portable context across harnesses/worktrees is the scarce power-user asset); and a **local replay/score of multi-agent runs** as the power-user substitute for a vendor benchmark.
- **Perspective:** biased to product-mechanics over positioning; undervalues distribution moats; treats blog "mechanics" as rumor (aligns with our downgrades).

### Cross-model synthesis — agreement with the baseline
- Both **reject blind parity** (the ICP-mismatch spine survives external scrutiny; Claude's steelman is explicitly weak).
- Both treat marketing mechanics as **rumor** (reinforces the EXTRACTED/INFERRED downgrades).
- Both keep a **vendor-benchmark SKIP** (Grok's "local replay-score" is a *refinement*, not a contradiction).
- Both name **ICP-overlap over the next ~12 months as THE hinge** (matches the open-questions list).

### Cross-model synthesis — where they force a change (the de-risking payoff)
1. **Moat-logic error (Claude).** Fix the "MCP open ⇒ weak moat" non-sequitur everywhere. Openness removes protocol lock-in, not execution moat — which also means *our* local-first execution (PTY reliability, attention-queue UX, harness glue) is a **real** moat, not just a feature list.
2. **Onboarding is not a beginner-only SKIP (Claude).** SKIP *beginner video courses*; ADD *power-user onboarding recipes* (per-harness autonomy profiles / playbooks). The activation curve is a power-user problem too.
3. **Don't SKIP coordination semantics with the kanban chrome (Grok).** Strengthen R-ORCH from "convention/adapter" toward a **durable ownership / claim-wait / assignment model**; keep drag-kanban SKIP.
4. **Monitoring = honest per-agent health, not a vanity dashboard (Grok).** Tighten R-OBS to real signals (process + git + last-tool-failure + human-gate queue depth); the differentiator is *honesty + granularity*, not KPI chrome (reinforces the no-fake-affordance rule).
5. **Memory = headline productized surface, not a soft borrow (Grok).** Upgrade R-MEM to a first-class, exportable, git-friendly, open-format agent-shared memory+skill layer.
6. **New axis — remote/multi-machine fleet (Claude).** Add R-REMOTE: a supervisory plane over remote workers. Crosses an infra/deployment boundary → treat as **human-design**.
7. **Confidence + kill-switch (both).** External confidence (62 / 42) is **lower and more conditional** than the baseline's tone. Hold the ICP-mismatch as a *working hypothesis with a 12-month re-review trigger*, not dogma; if BridgeMind moves upmarket, re-run the SKIP list.

## 11. Revisions these votes force on the earlier sections (punch-list)

To apply across `PRD.md` / `DESIGN-BRIEF.md` / `BUILD-PLAN.md`. ✅ = recommend apply now (low-regret / correctness); ⚠️ = strategic bet, recommend but your call; 🛑 = crosses a human-design boundary (design before build).

**Applied 2026-07-29:** items **1–4** (user decision `apply 1-4`) — folded into PRD / Design / Build and into §0 / §2 / §3 of this synthesis. Items 5–7 remain proposed (⚠️/🛑) pending a further decision.

1. ✅ **§2 / PRD moat framing** — replace "integration moat weak because MCP open" with the protocol-vs-execution distinction; add that local-first execution is a genuine moat.
2. ✅ **PRD §0/§3 + synthesis TL;DR** — add a **confidence + hinge** note (external 42–62; ICP-overlap is the hinge; 12-month re-review trigger; ICP-mismatch = hypothesis with kill-switch).
3. ✅ **R-OBS / monitoring** — reword the differentiator as *honest, granular per-agent health from real signals*; drop "fleet KPI dashboard" vanity framing; acceptance criteria = real process+git+tool-failure+queue-depth signals + visible stale/`state_blind` states.
4. ✅ **Onboarding** — change the learn-hub line from hard SKIP to *SKIP beginner courses / ADD power-user onboarding recipes* (new P1 **R-ONBOARD**: per-harness autonomy profiles + playbooks; docs, not video).
5. ⚠️ **R-MEM** — promote from "graph view nice-to-have" to a **first-class, exportable, git-friendly, open-format** agent-shared memory+skill surface (P1; candidate headline borrow).
6. ⚠️ **R-ORCH** — strengthen from convention to a **durable ownership / claim-wait / assignment model** (channel-B safe; the drag-kanban UI stays SKIP).
7. 🛑 **R-REMOTE (new)** — supervisory plane over remote/SSH/daemon workers reporting into the queue — **REQUIRES-HUMAN-DESIGN** (infra/deployment boundary); P2 until designed.

**Net:** the cross-LLM pass did **not** overturn the spine (both external models reject blind parity), but it corrected a moat-logic error, converted two hard SKIPs into nuanced borrows, promoted memory to a headline feature, surfaced a remote-fleet axis, and lowered confidence with an explicit kill-switch — i.e. exactly the de-risking the pass was run for.

## 12. Sources

Official (via indexed snippets / OSS, **not** live-fetchable): bridgemind.ai homepage; /products/bridgespace; /products/bridgevoice; /bridgeswarm; /bridgemcp; /opensource; /bridgebench; /products/bridgeagent; /pricing; /roadmap; /changelog; docs.bridgemind.ai (getting-started, bridgespace, bridgevoice, mcp); github.com/bridge-mind (+ bridgebench, MIT); bridgebench.ai.
Third-party: starsearn.com/guides/bridgemind-review-2026 (+ alternatives, vs-cursor); vibecademy.ai (blog + case study); augmentcode.com/guides/multi-agent-cost-compounding; pubroot.com coordination-protocol & three-tier articles (**community/ecosystem — INFERRED attribution only**); ViBench (ACM CAIS 2026, doi 10.1145/3786335.3813162); Vals Vibe Code Bench (vals.ai).
Internal ground truth (harness-ready): ui/HANDOFF.md; ui/README.md; ui/AGENTS.md; Cargo.toml; docs/REQUIRES-HUMAN-DESIGN-liveness-blindness.md; docs/REQUIRES-HUMAN-DESIGN-branch-wire-through.md; COMMAND + MONITORING screenshots.
Vault: ~/Memory/personal/topics/orchestration-market-landscape-2026.md; ~/Memory/personal/entities/agent-teams-entity.md; ~/Memory/personal/handovers/2026-06-18-agent-teams-bridgemind-ui-retheme.md; polyglot-broker SKILL.md + references/domains.md.
Persisted synthesis note (agent-teams MCP memory): `mem_1785335897022_33352_0`.
