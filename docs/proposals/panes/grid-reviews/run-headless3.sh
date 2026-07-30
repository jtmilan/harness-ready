#!/usr/bin/env bash
# Grounded headless for the harnesses that need auto-tools: throwaway worktree (isolates writes)
# + per-process GIT_CONFIG pushurl-neuter (push -> /dev/null, so --yolo/--trust cannot push) +
# auto-tools read the PR code via git show/diff. codex stays read-only (no writes anyway).
# Capture 2>&1 so streamed reasoning counts as the grounded review even if a long run is cut.
set -u
REPO=/Users/jeffrymilan/Personal/harness-ready
cd "$REPO"
OUT=docs/proposals/panes/grid-reviews
LOG=/tmp/hr3.log; : > "$LOG"
git fetch jtmilan >/dev/null 2>&1 || true
RULES='HONESTY RULES: R3 no fake affordance. R4 never simulated-as-live. R5 lanes theme-independent. R6 local-first. R7 palette cmd has non-palette route. R8 color never sole signal. R10 no contract change as code; no new deps. Rust OFF-by-default invariant. ANTI-HALLUCINATION: only report what you can verify by reading; never fabricate line numbers or code. OUTPUT one line per finding <branch-or-file>:<line>: <BLOCKING|SHOULD|NIT>: <problem>. <fix>. then VERDICT: b/s/n then ## BOUNDARIES.'
BR='Read the changes via git show jtmilan/<branch>:<path> or git diff main..jtmilan/<branch> for: phase/2-templates-local phase/5-palette phase/1b-outcomes phase/3-context phase/6-skills phase/0-unification phase/7-shared-state phase/fix-daemon-registry-test. Your git push is disabled and you are in a throwaway worktree, so do NOT modify the main repo at /Users/jeffrymilan/Personal/harness-ready.'
L_CURSOR='ROLE=reviewer LENS=typescript/react. Check hooks rules, re-render hygiene, a11y of new controls, pure helpers used+tested, the palette commands array is a useMemo array not a function.'
L_CMD='ROLE=reviewer LENS=rust. Check ownership, no unwrap on IO, OFF-by-default invariant (flag-off path equals prior logic), no unsafe, no new crate dep, no app/daemon edits, pure core unit-tested, events.jsonl scan path-confined.'
L_OPENCODE='ROLE=reviewer LENS=architecture. Check ui-to-bridge single-contract boundary (R10), NEEDS-BACKEND seams isolated as notes not fake impls, reconciler = pure core + thin IO shell, stacked-PR topology coherence.'
L_PI='ROLE=reviewer LENS=general/infra. Check minimal diff, consistency with repo precedents, monitoring honesty (empty+bannered, no fake numbers).'
NEUTER="GIT_CONFIG_COUNT=2 GIT_CONFIG_KEY_0=remote.jtmilan.pushurl GIT_CONFIG_VALUE_0=/dev/null GIT_CONFIG_KEY_1=remote.origin.pushurl GIT_CONFIG_VALUE_1=/dev/null"

run_yolo() { # $1=name $2=branch-for-worktree $3=cli-with-flags $4=brief
  local name="$1" br="$2" cli="$3" brief="$4"
  local wt; wt=$(mktemp -d)
  git worktree add -f -d "$wt" "jtmilan/$br" >>"$LOG" 2>&1 || true
  echo "##### $name START $(date +%T) wt=$wt" >>"$LOG"
  ( cd "$wt" && env $NEUTER timeout 300 $cli "$brief $RULES $BR" ) > "$OUT/headless-$name.md" 2>&1
  echo "rc=$?" >>"$LOG"
  git worktree remove -f "$wt" >>"$LOG" 2>&1 || true
  echo "##### $name END $(date +%T) bytes=$(wc -c < "$OUT/headless-$name.md" 2>/dev/null)" >>"$LOG"
}

# codex: read-only sandbox isolates writes; capture 2>&1; longer timeout for the rust volume
echo "##### codex-testing START $(date +%T)" >>"$LOG"
( timeout 480 codex exec --sandbox read-only "ROLE=reviewer LENS=testing. $RULES $BR Ensure new pure helpers (commandPalette, skillsManifest, contextGraph, sharedState, orchPaths, monitorRows, templateIO) have edge-case tests; flag honesty gaps." ) > "$OUT/headless-codex-testing.md" 2>&1
echo "codex rc=$?" >>"$LOG"

run_yolo cursor-typescript  main "cursor-agent --yolo -p" "$L_CURSOR" &
run_yolo commandcode-rust   main "commandcode --yolo -p" "$L_CMD" &
run_yolo opencode-architecture main "opencode run" "$L_OPENCODE" &
run_yolo pi-general         phase/1a-monitoring-tokens "pi -p --tools read,grep,find,ls,bash,edit,write" "$L_PI" &
wait
echo "ALL DONE $(date +%T)" >>"$LOG"
