#!/usr/bin/env bash
# Remaining five harnesses, PARALLEL + short timeouts (bounded; foreground; sandboxed; no yolo).
set -u
REPO=/Users/jeffrymilan/Personal/harness-ready
cd "$REPO"
OUT=docs/proposals/panes/grid-reviews
LOG=/tmp/hr2b.log; : > "$LOG"
git fetch jtmilan >/dev/null 2>&1 || true
RULES='HONESTY RULES: R3 no fake affordance. R4 never simulated-as-live. R5 lanes theme-independent. R6 local-first. R7 palette cmd has non-palette route. R8 color never sole signal. R10 no contract change as code; no new deps. Rust OFF-by-default invariant. ANTI-HALLUCINATION: only report what you can read; if you cannot read the code write exactly COULD NOT ACCESS under ## BOUNDARIES and VERDICT: 0/0/0 - clean (no access). Never fabricate. OUTPUT one line per finding <branch-or-file>:<line>: <BLOCKING|SHOULD|NIT>: <problem>. <fix>. then VERDICT: b/s/n then ## BOUNDARIES.'
L_CODEX='ROLE=reviewer LENS=testing. Read changes via git show jtmilan/<branch>:<path> or git diff main..jtmilan/<branch>. Ensure new pure helpers have edge-case tests; flag honesty gaps.'
L_PI='ROLE=reviewer LENS=general/infra. This worktree is at phase/1a-monitoring-tokens. Review monitoring honesty + minimal diff.'
L_CURSOR='ROLE=reviewer LENS=typescript/react. Changes on phase/1a-monitoring-tokens phase/5-palette phase/1b-outcomes phase/3-context phase/6-skills. If you cannot read them, COULD NOT ACCESS.'
L_CMD='ROLE=reviewer LENS=rust. Changes on phase/0-unification phase/7-shared-state phase/fix-daemon-registry-test. If you cannot read them, COULD NOT ACCESS.'
L_OPENCODE='ROLE=reviewer LENS=architecture. Changes on phase/0-unification phase/7-shared-state phase/3-context. If you cannot read them, COULD NOT ACCESS.'
BR='Branches: phase/2-templates-local phase/5-palette phase/1b-outcomes phase/3-context phase/6-skills phase/0-unification phase/7-shared-state phase/fix-daemon-registry-test. Read-only only; do not modify or push.'

( timeout 240 codex exec --sandbox read-only "$L_CODEX $RULES $BR" > "$OUT/headless-codex-testing.md" 2>>"$LOG"; echo "codex rc=$?" >>"$LOG" ) &
git worktree add -f /tmp/wt_pi jtmilan/phase/1a-monitoring-tokens >>"$LOG" 2>&1 || true
( timeout 150 bash -c "cd /tmp/wt_pi && pi -p --tools read,grep,find,ls '$L_PI $RULES'" > "$OUT/headless-pi-general.md" 2>>"$LOG"; echo "pi rc=$?" >>"$LOG" ) &
( timeout 150 cursor-agent -p "$L_CURSOR $RULES $BR" > "$OUT/headless-cursor-typescript.md" 2>>"$LOG"; echo "cursor rc=$?" >>"$LOG" ) &
( timeout 150 commandcode -p "$L_CMD $RULES $BR" > "$OUT/headless-commandcode-rust.md" 2>>"$LOG"; echo "commandcode rc=$?" >>"$LOG" ) &
( timeout 150 opencode run "$L_OPENCODE $RULES $BR" > "$OUT/headless-opencode-architecture.md" 2>>"$LOG"; echo "opencode rc=$?" >>"$LOG" ) &
wait
git worktree remove -f /tmp/wt_pi >>"$LOG" 2>&1 || true
echo "ALL DONE $(date +%T)" >>"$LOG"
