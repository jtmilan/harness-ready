# PRD — Absorbing BridgeMind-worthy Capabilities into harness-ready

**Status:** Draft (research-complete, pre-implementation)
**Date:** 2026-07-29
**Owner:** Coordinator synthesis — 4 deep-dive lenses (product-surface, orchestration/architecture, commercial/moat, ecosystem/benchmark) + 1 adversarial review, domain skillsets assigned via the polyglot-broker closed-enum catalog.
**Target product:** `harness-ready` (`/Users/jeffrymilan/Personal/harness-ready`) — the macOS "Agent Command Center".
**Subject of analysis:** https://www.bridgemind.ai/ (BridgeMind / BridgeSpace).

> **Provenance & honesty notice (read first).** The BridgeMind live site sits behind a Cloudflare JS challenge; `WebFetch` returns 403 and `curl` returns the "Just a moment" interstitial. **No claim in this document is extracted from the live DOM.** BridgeMind product detail is sourced from indexed sub-pages, the official `/pricing` page snippets, the open-source GitHub org, and third-party reviews (notably starsearn.com), and is tagged per the vault honesty convention: **EXTRACTED** (a fetched/searched source that is actually *about BridgeMind* states it), **INFERRED** (reasoned from evidence), **AMBIGUOUS** (unverified — do not build as if fact). Where an earlier lens over-attributed a *community/ecosystem* article (e.g. pubroot.com coordination write-ups that merely *name* BridgeMind as one example) as a BridgeMind internal, the adversarial pass downgraded it EXTRACTED → INFERRED; those downgrades are applied throughout. The `/polyglot-broker` and `/graphify` *skills* were not installed in this environment; the broker's closed 11-tag enum was applied manually (catalog read verbatim, no live fetch) and graphify was used via its CLI. See `~/.claude/context/2026-07-29-bridgemind-competitive-deepdive.md` for the pre-research brief.

---

## 1. Executive summary

BridgeMind is a **desktop-native Agentic Development Environment (BridgeSpace)** [EXTRACTED: products/bridgespace + changelog] that wraps multi-agent coding (BridgeSwarm), persistent project memory (BridgeMemory), an MCP server (BridgeMCP), voice dictation (BridgeVoice), a kanban dispatch board (BridgeBoard), and an open-source benchmark (BridgeBench) into one window, sold as a credit-based SaaS ($16–$100/mo) to **non-technical / semi-technical "vibe coding" MVP builders** [EXTRACTED: starsearn review; "Cursor recommended for experienced developers" — explicit exclusion].

harness-ready serves a **different ICP**: terminal-native power users who *orchestrate* coding agents (8 CLI harnesses, PTY + automatic git-worktree isolation, a ranked "who-needs-you" attention queue, fine-grained autonomy gates, fleet telemetry). **The single most important finding of this analysis is the ICP mismatch.** Chasing BridgeMind feature-for-feature would import beginner-ICP dead weight (drag-kanban, voice, tutorial hub, vendor benchmark, app themes) into a power-user tool. This PRD therefore does **not** pursue parity. It pursues a *filtered* borrow: adopt the capabilities that genuinely serve power users, *adapt* the patterns that need reshaping, and *skip* the rest — while protecting and amplifying harness-ready's existing differentiators.

**North-star decisions (the spine of every requirement below):**
1. **Borrow** persistent-context ideas (BridgeMemory) and the open-source-skills *posture* — both already close in harness-ready.
2. **Adapt** the role-cast orchestration *pattern* and multi-pane UX — keep harness-ready's harness-agnostic, PTY-level implementation.
3. **Skip** BridgeBoard drag-dispatch, BridgeVoice, the learn/content hub, a vendor benchmark, and app themes — beginner-ICP features.
4. **Make real** the MONITORING tab — BridgeMind has *no* fleet telemetry UI [adversarial CONFIRMED], so this is a harness-ready **differentiator**, not catch-up work.
5. **Do not build toward mirages** — the "cloud-first architecture", "automatic worktree-per-agent", "container sandbox per worktree", and "three modes" claims are refuted/un-evidenced for BridgeMind (see §8 watchlist).

---

## 2. Problem & opportunity

harness-ready already wins on the axes power users care about: local-first (no vendor cloud lock-in), true subprocess control (PTY, not MCP-mediated), automatic git-worktree isolation, a ranked attention queue, autonomy gates, and an 8-harness breadth that includes terminal-native CLIs BridgeMind ignores (OpenCode, CommandCode, Pi, Grok, bash). [EXTRACTED: ui/HANDOFF.md, Cargo.toml, screenshots.]

The gaps that *do* matter for this ICP, and that BridgeMind's success highlights, are: (a) monitoring is currently **simulated** (`ui/src/lib/monitorData.js`, `MockAgentBridge`) [EXTRACTED: ui/HANDOFF.md]; (b) templates persist via the **Base44** entity rather than a local store [EXTRACTED: ui/HANDOFF.md]; (c) two trust-critical items remain open **REQUIRES-HUMAN-DESIGN** gates — *liveness-blindness* (read-path vs mutation-path liveness diverge; `live.json` cleared on startup; multi-instance clobber) and *branch-wire-through* [EXTRACTED: docs/REQUIRES-HUMAN-DESIGN-liveness-blindness.md]; (d) persistent cross-session context exists (the `memory` crate + MCP memory tools) but lacks the *compounding, graph-shaped, always-available* quality that makes BridgeMemory a retention hook; (e) no public content/learn surface or published skills catalog, which is a distribution/trust lever even for power users.

The opportunity is to close (a)–(e) on *our* terms — using BridgeMind as evidence of *what* matters, not *how* to copy it.

---

## 3. Target user & ICP (and the contrast that governs scope)

| | harness-ready ICP (this PRD's user) | BridgeMind ICP (for contrast) |
|---|---|---|
| Who | Terminal-native devs / power users orchestrating coding-agent fleets | Non-technical & semi-technical founders / indie hackers shipping MVPs without coding |
| Mental model | Subprocesses, worktrees, queues, gates, diffs | "Ask → Get → Done", drag a card, watch it build |
| Wants | Subprocess control, isolation, observability, autonomy ceilings, harness choice | A managed experience that hides the terminal |
| Evidence | 8 CLI harnesses incl. bash; PTY; autonomy gates; attention queue [EXTRACTED: HANDOFF] | "Optimized for beginners, entrepreneurs, independent creators"; "Cursor recommended for experienced developers" [EXTRACTED: starsearn] |
| Willingness to pay | BYO-API-key / local tooling | Credit SaaS, $16–$100/mo |

**Implication:** a feature that primarily reduces *terminal anxiety* (voice, drag-kanban, tutorials, themes) has low value here; a feature that improves *control, isolation, memory, or observability* has high value. The §6 matrix applies this test.

**Confidence & kill-switch (cross-LLM review, applied 2026-07-29, item 2):** the ICP-mismatch framing above is a *working hypothesis*, not dogma. Independent external-model adversarial votes (Anthropic 62, xAI 42 / 100) named **ICP overlap over the next ~12 months as the single hinge**: if BridgeMind moves upmarket, or the same operators use both a vibe ADE and a CLI fleet, re-run the §6 SKIP list. Hold the spine as monitored, with a 12-month re-review trigger.

---

## 4. Competitive landscape & honest moat read

### 4.1 Positioning map

| Product | Layer | Local/Cloud | Multi-agent | Openness | Notes |
|---|---|---|---|---|---|
| **BridgeMind** | ADE / orchestrator | Desktop app + MCP bridge | Yes (BridgeSwarm) | MIT benchmark + OSS skills | Real comp set = Lovable/Bolt/Replit (non-coder MVP), *not* Cursor [INFERRED from review clustering + explicit "experienced devs use Cursor"] |
| Cursor | IDE | Local | Partial (background agents) | Closed | Wins experienced-dev IDE niche |
| Cline / Aider | Editor ext / CLI | Local | Single-agent loop | OSS, BYOK | Cost + control leaders |
| Devin (Cognition) | Autonomous engineer | Cloud sandbox | Yes | Closed | Autonomy leader; price dropped $500→$20/mo [EXTRACTED] |
| Bolt.new / Lovable / v0 | App generators | Cloud | No | Closed | Lovable ~$6.6B val / ~$200M ARR [EXTRACTED] — the capitalised end of BridgeMind's ICP |
| Replit Agent | Cloud IDE+runtime | Cloud | No | Closed | Backend-runtime leader |
| LangGraph / CrewAI / AutoGen | Frameworks | DIY | Yes | OSS | Different ICP (builders of agent systems); monetise via observability/runtime |
| **harness-ready** | Agent command center | Local | Yes (pane model) | OSS harnesses | Power-user orchestrator; local-first |

### 4.2 BridgeMind moat — honest read (not sandbagged)
**Narrow and media-weighted.** Durable assets: (1) founder-led *build-in-public* distribution (Matthew Miller; YouTube ~97k subs, TikTok, X ~24k, Discord ~7–8k; ARR ~$245–250k around day ~209 — **founder self-reported, un-audited** [EXTRACTED/AMBIGUOUS]); (2) per-user BridgeMemory graphs. **Weak at the *protocol* layer:** technology (orchestrator over third-party LLMs, no proprietary model) and integrations (replicable); and BridgeMCP/BridgeMemory create no *protocol* lock-in because MCP is open *by design* (any client reads the same memory). **Correction (cross-LLM review, applied 2026-07-29, item 1):** openness removes *protocol* lock-in, **not** *execution* moat — an open standard coexists with strong implementation moats (OCI is open; Docker/Kubernetes remain moated). So the honest read is: BridgeMind's *protocol/integration* moat is weak, but its *execution* moat (harness glue, PTY/loop reliability, attention-queue UX) is real-if-unproven — and symmetrically, **harness-ready's local-first execution is itself a genuine moat**, not merely a feature list. [INFERRED + EXTRACTED; correction INFERRED from a cross-LLM adversarial vote.] **Takeaway for the PRD:** do *not* copy BridgeMind's media strategy or over-invest in defensibility-by-feature; compete on technical differentiation (observability, autonomy gates, harness-agnostic orchestration, local-first execution-as-moat).

### 4.3 Pricing (for reference, not emulation)
Basic $20/mo ($16 annual) / 5,000 credits; Pro $50/mo ($40) / 12,500 credits + BridgeMemory/MCP/Voice; Ultra $100/mo ($80) / 25,000 credits. **No free tier; 7-day money-back.** Per-action credit schedule **undisclosed** [EXTRACTED: /pricing + starsearn; AMBIGUOUS: unit economics]. harness-ready's BYO-key/local model sidesteps the credit-compounding trap entirely (a 3–5 agent swarm on BridgeMind's opaque credits is a documented backlash vector elsewhere in the market [INFERRED: augmentcode credit-compounding analysis]) — keep it that way.

---

## 5. BridgeMind feature & surface spec (the "does it have API / settings / menus?" answer)

This section answers the explicit spec question, with evidence tags.

### 5.1 Feature inventory (categorized)
- **Orchestration — BridgeSwarm:** one prompt → coordinated team; closed roles **Coordinator / Builder / Scout / Reviewer**; per-role model selection; live "mission tree" to steer; marketing says up to 16 agents in BridgeSpace. [EXTRACTED: bridgeswarm page + blog; role set consistent across lenses.] *Caveat:* the detailed mechanics often cited (a 6-column `SWARM_BOARD.md`, `bs-mail` 30–60s pull messaging, a `CLAIM/WAIT/RELEASE` mutex, a `BCL` queue, 3 concurrent coordinators) come from **pubroot.com community/ecosystem articles** that name BridgeMind as *one* deployment — treat as **INFERRED**, not BridgeMind-internal fact.
- **Workspace — BridgeSpace:** desktop ADE; multi-pane terminals (1–16) + native code editor + BridgeBoard kanban + mission tree; "command blocks" (Warp-style). [EXTRACTED: products/bridgespace.] "Three modes. One platform." is marketed but the mode names are **AMBIGUOUS** (not enumerated in indexed sources).
- **Memory — BridgeMemory:** per-project `.bridgememory/` markdown knowledge graph, MCP-native, compounds across sessions; **gated at Pro+**. [EXTRACTED: blog + pricing.]
- **Integration — BridgeMCP:** MCP server = persistent kanban + shared context + a `taskKnowledge` field; aggregates upstream MCP servers into "virtual endpoints"; editor clients (Cursor, Claude Code, Windsurf, Cline, Codex, Gemini CLI, Hermes, OpenClaw) connect as MCP clients. [EXTRACTED: bridgemcp + docs; integration list EXTRACTED from homepage/opensource; AMBIGUOUS: read-vs-write gating of BridgeMCP.]
- **Voice — BridgeVoice:** push-to-talk + toggle dictation, 99+ languages, ~150 WPM, custom dictionary, system-wide (into any desktop app), on-device option implied by "privacy-first". [EXTRACTED: products/bridgevoice + docs.]
- **Kanban — BridgeBoard:** in-BridgeSpace kanban with **two-way sync** (drag card → agent picks up; agents move cards through a review column). [EXTRACTED: products/bridgespace.]
- **Benchmark — BridgeBench:** OSS (MIT), 130+ tasks, v2 = 7 categories, v3 = blind three-judge panel + Elo, tracks speed + cost + quality; `github.com/bridge-mind/bridgebench`. [EXTRACTED: bridgebench + github; AMBIGUOUS: task provenance, judge-model pinning, independent reproduction.]
- **Code — BridgeCode:** a fork of OpenCode for vibe coding. [EXTRACTED.]
- **OSS skills:** `github.com/bridge-mind` — BridgeSecurity, BridgeWard, BridgeSpeak, BridgeBench; "compatible with 30+ tools". [EXTRACTED: opensource page.]
- **BridgeAgent:** marketed "recursive AI software engineer", "plugs into 25+ tools" (**AMBIGUOUS**: not enumerated).

### 5.2 Settings / preferences surface (concrete)
**Evidenced** [EXTRACTED]: Settings → **API Keys** (generate named keys, e.g. "Cursor MCP", "Claude"); **theme picker** in nav (Paper / Chalk / Solar / Arctic / Ivory); **BridgeVoice → Widget Appearance** ("auto-hide widget when not recording"); dashboard **credit balance**; cookie preferences (auth/session/CSRF/analytics).
**Not evidenced (AMBIGUOUS)** — and notably *absent vs harness-ready*: trusted-repositories allowlist; autonomy / permission gates (no analogue of `allow_mutations` / `autonomy_ceiling`); notification / alert routing; keyboard-shortcut customization; per-session memory toggles; billing-management UI detail.

### 5.3 Developer / API surface
**Evidenced** [EXTRACTED]: **BridgeMCP** server + API reference at docs; **API-key flow** documented; **editor MCP integrations** configured via each editor's command palette (e.g. `Windsurf: Configure MCP Servers`); **MIT open-source skills** on GitHub.
**Not evidenced (AMBIGUOUS)**: any public **REST/GraphQL API**, **webhooks**, or a **standalone BridgeMind CLI** (BridgeCode is an OpenCode fork, not a BridgeMind CLI). *Takeaway:* BridgeMind's "API" story **is** MCP + keys + OSS skills — there is no conventional REST surface to copy.

### 5.4 Menus / information architecture
**Evidenced** [EXTRACTED]: BridgeSpace window = terminals grid + code editor + BridgeBoard + mission tree + review context; a **command palette** (`Cmd/Ctrl+Shift+P`, seen used for MCP config); Settings entries above; theme picker in nav; dashboard.
**Not evidenced (AMBIGUOUS)**: top-nav/tab/kebab structure specifics, empty-states, a dedicated memory/monitoring/fleet-KPI pane. *Notably:* **no fleet telemetry / CPU-mem-success dashboard exists** [adversarial CONFIRMED via zero-result search] — harness-ready's MONITORING tab is therefore a **differentiator**.

### 5.5 Core UX flows (as evidenced)
Spawn = open terminal panes (exact spawn control **AMBIGUOUS**); orchestrate = BridgeSwarm single-prompt → role-cast team + mission tree; dispatch = BridgeBoard drag / two-way sync; monitor = mission tree + per-pane terminals (no fleet KPIs); review = BridgeBoard review column + a stated "a person checked every change before it shipped" philosophy (explicit approve/reject button UX **AMBIGUOUS**); voice = push-to-talk into any app.

---

## 6. Feature transfer decision matrix (authoritative — governs §7)

Test applied: *does this serve a terminal-native power user, or only a beginner?*

| BridgeMind capability | Evidence | Fits our ICP? | Decision | Rationale |
|---|---|---|---|---|
| Persistent context / memory graph (BridgeMemory) | EXTRACTED | Yes | **BORROW** | Power users value compounding context; our `memory` crate + MCP tools already cover the backend — add an always-available, graph-shaped view. |
| Open-source skills *posture* | EXTRACTED | Yes | **BORROW** | Validates publishing our `.claude/skills`; we already have the catalog — make it public-facing. |
| Role-cast orchestration (Coordinator/Builder/Scout/Reviewer) | EXTRACTED (roles); mechanics INFERRED | Yes (pattern) | **ADAPTER** | Take the *role-casting + file-ownership* idea; keep our harness-agnostic `team_orchestrate` + `roles` crate. Do not copy Cursor-centric swarm impl. |
| Multi-pane terminals | EXTRACTED | Yes | **ADAPTER** | Our PTY-level grid already exceeds their UI-level panes; no gap — keep/evolve ours. |
| Fleet telemetry / monitoring | **REFUTED for them** | Yes | **MAKE REAL (ours)** | They have none; we have a simulated tab — wiring it real is a differentiator, not parity. |
| Command palette | AMBIGUOUS | Marginal | **OPTIONAL** | Power users live in shells; a palette is convenience, not core. P2. |
| Drag-dispatch kanban (BridgeBoard) | EXTRACTED | No | **SKIP** | Terminal-native users dispatch via CLI/task-list/orchestrate; drag-UI is beginner-ICP. Our task lifecycle log suffices. |
| Voice dictation (BridgeVoice) | EXTRACTED | No | **SKIP** | Orthogonal to terminal workflow; low ROI for this ICP. |
| Learn / content hub | EXTRACTED | Partial | **SKIP** beginner courses / **ADD** onboarding recipes (applied 2026-07-29, item 4) | Beginner *video/tutorial* content = dead weight for this ICP; BUT 8 harnesses + fine-grained autonomy gates make **activation** a power-user problem too → ship **recipes/playbooks** (per-harness autonomy profiles), not courses. See R-ONBOARD. |
| Vendor benchmark (BridgeBench) | EXTRACTED (exists); credibility AMBIGUOUS | No | **SKIP** | Vendor-authored, self-dealing, reproducibility unverified; reference SWE-bench/ViBench instead. |
| App themes | EXTRACTED (their themes) | No | **SKIP** | Power users theme via shell/editor; (note: sibling agent-teams already has a skins system — not a harness-ready priority.) |
| "Three modes" | NOT evidenced | n/a | **SKIP** | Mirage; our autonomy gates already express modes. |
| Cloud-first architecture | REFUTED | n/a | **SKIP** | Mirage; we are intentionally local-first. |
| Container sandbox per worktree | REFUTED (no source) | Maybe later | **DEFER (human-design)** | Not a real BridgeMind feature to copy; if *we* want stronger isolation it's our own design, gated. |

---

## 7. Requirements

Legend: **P0** = do now (differentiator or trust-critical); **P1** = next; **P2** = opportunistic. **HD** = touches a mutation/schema/boundary → emits a REQUIRES-HUMAN-DESIGN note; do *not* implement speculatively.

### R-OBS — Real fleet monitoring (P0, differentiator)
- **Story:** as an operator running N agents, I see *real* per-process CPU/memory and *real* task success/outcome rates, not simulated curves.
- **AC:** `monitorData.js` + `MockAgentBridge` simulation removed from the live path; metrics sourced from `core/supervisor`/`core/daemon`; MONITORING KPIs + live chart + per-agent success bars reflect truth; **the differentiator is honest, granular per-agent health from *real* signals — process liveness + per-child CPU/mem + git state + last-tool-failure + human-gate queue depth (cross-LLM review, applied 2026-07-29, item 3) — not a vanity fleet dashboard**; a flatline or a `state_blind` harness renders as an explicit "telemetry limited / no data" state, never an interpolated or simulated curve; graceful empty state when no fleet.
- **Touches:** `ui/src/pages/Monitoring.jsx`, `ui/src/lib/monitorData.js`, `ui/src/lib/agentBridge.js` + tauri adapter, `core/supervisor`, `core/daemon`. **Effort:** M. **Depends:** none. **HD:** no.

### R-GATES — Close the two open trust gates (P0, HD)
- **Story:** the read path and the mutation path must agree on which panes are live; branch info must wire through cleanly.
- **AC:** liveness-blindness root causes (registry-vs-`sups` divergence, startup `live.json` clobber, multi-instance clobber, MCP queue filter) resolved or explicitly redesigned; branch-wire-through closed.
- **Touches:** `app/src-tauri/src/lib.rs`, `core/mcp`, `core/daemon`, `agent-teams-mcp`. **Effort:** L. **HD:** **YES** — design the single source of liveness before coding (per the existing REQUIRES-HUMAN-DESIGN notes).

### R-TEMPLATES — Local template store (P1)
- **Story:** team templates persist locally, independent of Base44, with the same schema.
- **AC:** `AgentTemplate` schema preserved; storage swapped from Base44 entity to local store (SQLite/JSON per `ui/HANDOFF.md`); save/launch flows unchanged in UX.
- **Touches:** `ui/src/components/command/templates/*`, a new local store module. **Effort:** S–M. **HD:** no (local schema, no public boundary).

### R-MEM — Compounding persistent context + graph view (P1, borrow)
- **Story:** project context compounds across sessions and is browsable as a graph, always available to agents via MCP.
- **AC:** extend the `memory` crate / MCP memory tools toward an always-on, linkable graph (align with the *idea* of BridgeMemory, local-first); add a read-only graph view in the UI; **never** gate it behind a paid tier (we're local/BYOK).
- **Touches:** `core/memory`, `core/mcp`, a new UI graph view. **Effort:** M. **HD:** possibly (if it changes the memory store's write contract) — confirm before coding.

### R-ORCH — Role-cast orchestration + file-ownership hints (P1, adapter)
- **Story:** when I orchestrate a team, I can cast panes into Coordinator/Builder/Scout/Reviewer roles and declare non-overlapping file ownership, so parallel workers don't collide.
- **AC:** `team_orchestrate`/`roles` support explicit role casting and a *file-ownership* field carried on the **task channel** (channel B) — not the persona SSOT (channel A stays injection-safe); orchestrate preview shows the cast + ownership before dispatch. We already provide automatic worktree isolation, so this *adds* the coordination convention on top.
- **Touches:** `core/roles`, `core/mcp` (orchestration prompt builder), `ui` spawn/orchestrate wizard. **Effort:** M. **HD:** no if channel-B only; **YES** if it touches persona SSOT (it must not).

### R-ONBOARD — Power-user onboarding recipes / playbooks (P1, borrow; applied 2026-07-29, item 4)
- **Story:** a new operator can onboard harness #3 and a sane autonomy profile without reading source — via one-click recipes, not video tutorials.
- **AC:** a small library of per-harness **autonomy profiles + playbooks** (what gates to set, how to wire the harness, a first-run recipe), surfaced in the Templates/Recipes manager and the command palette; versionable as local files; **no beginner video/course surface** (that stays SKIP).
- **Touches:** `ui` (recipes UI reusing the templates manager / palette), a local recipes store. **Effort:** S–M. **Depends:** none. **HD:** no.
- **Rationale (cross-LLM review):** the activation curve is existential for power-user tools too; this converts the old hard SKIP of the 'learn hub' into a targeted borrow that serves *this* ICP.

### R-SKILLS-OSS — Publish the skills catalog (P2, borrow-light)
- **Story:** our skill library is publicly browsable, signaling quality and enabling contribution.
- **AC:** a landing/index for `.claude/skills` (reuse polyglot-expert/broker artifacts where relevant); MIT where appropriate.
- **Touches:** docs/site, repo `.claude/skills`. **Effort:** S. **HD:** no.

### R-PALETTE — Command palette (P2, optional)
- **Story:** power users can jump to any action (spawn, orchestrate, focus pane, open diff) from one keystroke.
- **AC:** palette over existing handlers (no new backend); keyboard-first.
- **Touches:** `ui`. **Effort:** S–M. **HD:** no.

### R-MCP-RW — Optional read-write shared-state layer (P2, HD)
- **Story:** a BridgeMCP-*like* shared task/context layer that agents can read *and* write under gates, complementing our currently read-only MCP projection.
- **AC:** design the write boundary, gating, and injection-safety before any code; likely a new gated layer over `core/mcp`/`core/task`.
- **Touches:** `core/mcp`, `core/task`, `core/supervisor`. **Effort:** L. **HD:** **YES** — crosses the mutation boundary; this is exactly the channel-A/B + write-gate design the broker SKILL.md warns about.

---

## 8. Out of scope & vaporware watchlist

**Out of scope (SKIP, with reason):** BridgeBoard drag-dispatch kanban, BridgeVoice, beginner tutorial/**video** hub (power-user onboarding *recipes* are NOT skipped — see R-ONBOARD), vendor benchmark, app-theme system, "three modes", cloud-first architecture — see §6. These serve BridgeMind's beginner ICP or are mirages; building them would dilute harness-ready's power-user focus.

**Vaporware / mirage watchlist — do NOT build toward these as "BridgeMind parity":**
1. Cloud-first architecture — **REFUTED** (BridgeSpace is desktop-native).
2. Automatic git-worktree-per-agent — **overstated** (they ship worktree *UI controls* in B3; automatic isolation not evidenced; *we* already have it).
3. Container sandbox per worktree — **no source** (inferred from `/sandbox` norms).
4. "Three modes (vibe/agentic/manual)" — **not evidenced**.
5. BridgeMind-side fleet KPI dashboard — **does not exist** (so it is not a gap for us).

---

## 9. Differentiators to protect & amplify

Automatic git-worktree-per-agent; ranked who-needs-you attention queue; **real** fleet telemetry (once R-OBS lands); fine-grained autonomy gates (`allow_mutations`, `send_input_enabled`, `autonomy_ceiling`, `external_spawn_max_panes`); `broadcast` / `delegate` primitives; append-only audit log; 8-harness breadth incl. terminal-native CLIs; local-first / BYO-key (no credit-compounding trap, no vendor cloud lock-in). **Do not** dilute these by importing beginner-ICP affordances.

---

## 10. Success metrics

- R-OBS: monitoring shows real values; `monitorData.js`/`MockAgentBridge` no longer on the live path; zero "looks live but is fake" surfaces.
- R-GATES: a pane alive to `send_input` is visible to `team_get_queue`/`team_read_output` (liveness parity); branch chip reliable.
- R-MEM: context survives session restart and is graph-browsable; agents retrieve it via MCP without re-deriving.
- Adoption: power-user workflows (orchestrate → parallel worktrees → ranked queue → real metrics) measurably smoother; **no** regression in the local-first / autonomy-gate model.
- Guard metric: count of beginner-ICP features shipped = **0** (the SKIP list holds).

---

## 11. Risks & open questions

- **ICP drift:** the largest risk is quietly shipping BridgeMind's beginner features. Mitigation: every new feature must pass the §6 test in review.
- **ICP hypothesis hinge (cross-LLM review, applied 2026-07-29, item 2):** the ICP-mismatch spine is a working hypothesis with a 12-month re-review trigger (external-model confidence 42–62/100). If BridgeMind moves upmarket or operators cross over, re-run §6 (see §3).
- **Provenance ceiling:** because the live site is unreachable, several specs (command-palette scope, approve/reject UX, BridgeMCP read/write gating, "25+ tools", real credit burn-rate, BridgeMind failure/retry/rollback model) remain **AMBIGUOUS** — verify against a real browser/Google-cache before depending on them.
- **Human-design load:** R-GATES and R-MCP-RW cross mutation/schema boundaries; per coordinator guardrails they stop for design, not silent implementation.
- **Single-source pricing / self-reported ARR:** treat commercial figures as directional only.
- **Gaps in this analysis** (un-investigated, flagged by the adversarial pass): BridgeMCP tool-surface detail; BridgeMind harness diversity beyond marketing; real-world credit burn-rate; BridgeBench task coverage vs infra/db/security workloads; BridgeMind's failure/retry/rollback model.

---

## 12. Sources

Official (via indexed snippets / open-source, **not** live-fetchable): bridgemind.ai homepage; /products/bridgespace; /products/bridgevoice; /bridgeswarm; /bridgemcp; /opensource; /bridgebench; /products/bridgeagent; /pricing; /roadmap; /changelog; docs.bridgemind.ai (getting-started, bridgespace, bridgevoice, mcp); github.com/bridge-mind (+ bridgebench, MIT); bridgebench.ai.
Third-party: starsearn.com/guides/bridgemind-review-2026 (+ alternatives, vs-cursor); vibecademy.ai (blog + case study); augmentcode.com/guides/multi-agent-cost-compounding; pubroot.com coordination-protocol & three-tier articles (**community/ecosystem — INFERRED attribution only**); ViBench (ACM CAIS 2026, doi 10.1145/3786335.3813162); Vals Vibe Code Bench (vals.ai).
Internal (ground truth for harness-ready): ui/HANDOFF.md; ui/README.md; ui/AGENTS.md; Cargo.toml; docs/REQUIRES-HUMAN-DESIGN-liveness-blindness.md; docs/REQUIRES-HUMAN-DESIGN-branch-wire-through.md; screenshots of COMMAND + MONITORING tabs.
Vault: ~/Memory/personal/topics/orchestration-market-landscape-2026.md; ~/Memory/personal/entities/agent-teams-entity.md; ~/Memory/personal/handovers/2026-06-18-agent-teams-bridgemind-ui-retheme.md; polyglot-broker SKILL.md + references/domains.md.
