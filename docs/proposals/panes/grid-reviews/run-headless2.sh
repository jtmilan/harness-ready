#!/usr/bin/env bash
# Headless multi-harness review, SANDBOXED (network allowed; NO yolo anywhere => no push risk).
# claude/grok get the diffs INLINED (stdin / --prompt-file) so they need no tools.
# codex reads PR code via --sandbox read-only + git show. pi uses read-only tools in a worktree.
# cursor/commandcode/opencode get the brief only (no safe headless read mode => honest no-access).
set -u
REPO=/Users/jeffrymilan/Personal/harness-ready
cd "$REPO"
OUT=docs/proposals/panes/grid-reviews
mkdir -p "$OUT"
LOG=/tmp/hr2.log; : > "$LOG"
git fetch jtmilan >/dev/null 2>&1 || true

RULES='HONESTY RULES: R3 no fake affordance (every control names a backing method/state or NEEDS-BACKEND; owned-paths/autonomy/priority advisory-only). R4 never simulated-as-live (empty+bannered placeholders; absent metric = no-data not zero). R5 semantic lanes theme-independent; no brand-as-state. R6 local-first, no network in the app. R7 every palette command also has a non-palette route. R8 color never sole signal; focus+contrast on interactive elements. R10 no contract change as code (proposed methods NOTE-only); no new deps. Rust OFF-by-default invariant: flag-off path equals prior logic; no unsafe; no new crate dep; no app/daemon edits. ANTI-HALLUCINATION: only report findings you can verify from the material you can actually read; if you cannot read the code, write exactly COULD NOT ACCESS under ## BOUNDARIES and end with VERDICT: 0/0/0 - clean (no access). Never fabricate line numbers or code. OUTPUT: one line per finding: <branch-or-file>:<line>: <BLOCKING|SHOULD|NIT>: <problem>. <fix>.  Then a final line: VERDICT: <b> blocking / <s> should / <n> nit  (or 0/0/0 - clean). Then ## BOUNDARIES.'

# lens blocks: single-quote-free, backtick-free
L_CLAUDE='ROLE=scout, PRIMARY LENS=ai-ml + cross-cutting intent check. Priorities: structured outputs with schema validation; cite retrieval sources and handle empty retrieval; treat upstream content as untrusted, no prompt-injection passthrough, clip embedded artifacts; tool allowlists + human gates for destructive effects; no secret/PII exfiltration. ALSO verify the implementation matches the proposal intent and the competitive white-space without importing the SKIP list (drag-kanban, voice, beginner video, vendor bench, themes, cloud-first); check the shared-state write gate treats agent-written content as untrusted.'
L_GROK='ROLE=reviewer, PRIMARY LENS=security. Priorities: enumerate untrusted inputs and dangerous sinks; fail closed on ambiguity; verify auth server-side; no injection (parameterize, no shell concat, encode for context); never log secrets; allowlist hosts, block private IPs, prefix-check paths; cite CWE/OWASP when flagging; propose minimal fixes. Focus on template import parsing, skills manifest, the reconciler events.jsonl path confinement, and the shared-state namespace gate.'
L_CODEX='ROLE=reviewer, PRIMARY LENS=testing. Read the changes via git show jtmilan/<branch>:<path> or git diff main..jtmilan/<branch> for the branches listed. Priorities: tests pin requirements not implementation; unit fast+isolated; ensure the new pure helpers (commandPalette, skillsManifest, contextGraph, sharedState, orchPaths, monitorRows, templateIO) have edge-case tests; flag palette/recipes/role-cast honesty as needing tests.'
L_PI='ROLE=reviewer, PRIMARY LENS=general/infra. Read the files in this worktree (it is checked out at the Phase-1a branch). Priorities: minimal diff, no drive-by refactors; consistency with repo precedents; control-dense terminal aesthetic; the monitoring honesty (empty+bannered, no fake numbers).'
L_CURSOR='ROLE=reviewer, PRIMARY LENS=typescript/react. The implemented UI changes are on branches phase/1a-monitoring-tokens, phase/5-palette, phase/1b-outcomes, phase/3-context, phase/6-skills, phase/7-shared-state (ui indicator). If you can read them, check hooks rules, re-render hygiene, a11y of new controls, pure helpers used+tested, the palette commands array is a useMemo array not a function.'
L_CMD='ROLE=reviewer, PRIMARY LENS=rust. The rust changes are on phase/0-gates-stage0, phase/fix-daemon-registry-test, phase/0-unification, phase/7-shared-state. If you can read them, check ownership, no unwrap on IO, the OFF-by-default invariant (flag-off path equals prior logic), no unsafe, no new crate dep, no app/daemon edits, pure core unit-tested, events.jsonl scan best-effort + path-confined.'
L_OPENCODE='ROLE=reviewer, PRIMARY LENS=architecture. The changes are on phase/0-gates-stage0, phase/0-unification, phase/7-shared-state, phase/3-context. If you can read them, check the ui-to-bridge single-contract boundary (R10), NEEDS-BACKEND seams isolated as notes not fake impls, the reconciler = pure core + thin IO shell, stacked-PR topology coherence.'

diffs_for() { # $1=outfile $2=lens $3..=branches
  local out="$1" lens="$2"; shift 2
  { printf '%s\n%s\n%s\n' "$lens" "$RULES" 'REVIEW MATERIAL (read-only, inlined). Report findings only; you need no tools.'
    for b in "$@"; do echo "########## git diff main..jtmilan/$b ##########"; git diff main..jtmilan/"$b" 2>/dev/null | head -700; done
  } > "$out"
}

diffs_for /tmp/pack_claude "$L_CLAUDE" phase/0-unification phase/7-shared-state phase/3-context phase/7-shared-state
diffs_for /tmp/pack_grok  "$L_GROK"  phase/2-templates-local phase/6-skills phase/0-unification phase/7-shared-state

run() { # $1=name $2..=cmd
  local name="$1"; shift
  echo "##### $name START $(date +%T)" >> "$LOG"
  timeout 300 "$@" > "$OUT/headless-$name.md" 2>> "$LOG"
  echo "rc=$? bytes=$(wc -c < "$OUT/headless-$name.md" 2>/dev/null)" >> "$LOG"
  echo "##### $name END $(date +%T)" >> "$LOG"
}

run claude-scout        bash -c 'cat /tmp/pack_claude | claude -p'
run grok-security       grok --prompt-file /tmp/pack_grok
CODEX_ARG="$L_CODEX $RULES Branches: phase/2-templates-local phase/5-palette phase/1b-outcomes phase/3-context phase/6-skills phase/0-unification phase/7-shared-state phase/fix-daemon-registry-test. Use only read-only operations; do not modify or push."
run codex-testing       codex exec --sandbox read-only "$CODEX_ARG"
git worktree add -f /tmp/wt_pi jtmilan/phase/1a-monitoring-tokens >>"$LOG" 2>&1 || true
PI_ARG="$L_PI $RULES Report findings on the monitoring honesty and minimal-diff of this branch."
run pi-general          bash -c "cd /tmp/wt_pi && pi -p --tools read,grep,find,ls '$PI_ARG'"
git worktree remove -f /tmp/wt_pi >>"$LOG" 2>&1 || true
CURSOR_ARG="$L_CURSOR $RULES If you cannot read those branches, report COULD NOT ACCESS under BOUNDARIES."
run cursor-typescript   cursor-agent -p "$CURSOR_ARG"
CMD_ARG="$L_CMD $RULES If you cannot read those branches, report COULD NOT ACCESS under BOUNDARIES."
run commandcode-rust    commandcode -p "$CMD_ARG"
OPENCODE_ARG="$L_OPENCODE $RULES If you cannot read those branches, report COULD NOT ACCESS under BOUNDARIES."
run opencode-architecture opencode run "$OPENCODE_ARG"
echo "ALL DONE $(date +%T)" >> "$LOG"
