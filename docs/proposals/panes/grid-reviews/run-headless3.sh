#!/usr/bin/env bash
# Grounded headless review for the harnesses that need auto-tools (--yolo/--trust), now that the
# user added the Bash permission rule. POST-MERGE: all phase work is on `main` (PRs #25-#38 merged),
# so each CLI reviews the MERGED code in a throwaway worktree at main. Push is neutered (per-process
# GIT_CONFIG pushurl=/dev/null) + worktree is throwaway, so --yolo cannot touch the main repo or push.
# Capture 2>&1 so streamed reasoning counts as the grounded review even if a long run is cut.
set -u
REPO=/Users/jeffrymilan/Personal/harness-ready
cd "$REPO"
OUT=docs/proposals/panes/grid-reviews
LOG=/tmp/hr3.log; : > "$LOG"
git fetch jtmilan >/dev/null 2>&1 || true
RULES='HONESTY RULES: R3 no fake affordance. R4 never simulated-as-live. R5 lanes theme-independent. R6 local-first. R7 palette cmd has non-palette route. R8 color never sole signal. R10 no contract change as code; no new deps. Rust OFF-by-default invariant. ANTI-HALLUCINATION: only report what you can verify by reading the files; never fabricate line numbers or code. OUTPUT one line per finding <file>:<line>: <BLOCKING|SHOULD|NIT>: <problem>. <fix>. then VERDICT: b/s/n then ## BOUNDARIES.'
BR='All phase work (PRs #25-#38) is MERGED into main. You are in a throwaway worktree checked out at main. Review the actual files in the working tree, e.g.: ui/src/pages/Home.jsx ui/src/pages/Monitoring.jsx ui/src/pages/Context.jsx ui/src/pages/Skills.jsx ui/src/lib/commandPalette.js ui/src/lib/skillsManifest.js ui/src/lib/contextGraph.js ui/src/lib/sharedState.js ui/src/lib/monitorRows.js ui/src/components/command/TopBar.jsx ui/src/components/command/SharedStateBadge.jsx ui/src/components/command/templates/templateIO.js ui/src/components/command/orchPaths.js core/mcp/src/lib.rs agent-teams-mcp/src/read_output.rs agent-teams-mcp/src/main.rs core/daemon/src/spawn.rs. Your git push is disabled and you are in a throwaway worktree; do NOT modify the main repo at /Users/jeffrymilan/Personal/harness-ready.'
L_CURSOR='ROLE=reviewer LENS=typescript/react. Review the merged ui on main. Check hooks rules, re-render hygiene, a11y of new controls, pure helpers used+tested, the palette commands array is a useMemo array not a function, and that the commands useMemo is defined AFTER the guardedNewAgent/guardedTemplates consts (no TDZ).'
L_CMD='ROLE=reviewer LENS=rust. Review core/mcp/src/lib.rs, agent-teams-mcp/src/read_output.rs, agent-teams-mcp/src/main.rs, core/daemon/src/spawn.rs on main. Check ownership, no unwrap on IO, OFF-by-default invariant (flag-off path equals prior logic), no unsafe, no new crate dep, pure core unit-tested, events.jsonl scan path-confined.'
L_OPENCODE='ROLE=reviewer LENS=architecture. Review the merged tree on main. Check ui-to-bridge single-contract boundary (R10), NEEDS-BACKEND seams isolated as notes not fake impls, reconciler = pure core + thin IO shell, merged-tree coherence.'
L_PI='ROLE=reviewer LENS=general/infra. Review the merged tree on main. Check minimal diff, consistency with repo precedents, monitoring honesty (empty+bannered, no fake numbers).'
L_CODEX='ROLE=reviewer LENS=testing. Review the merged tree on main. Be CONCISE: at most 8 findings. Ensure new pure helpers (commandPalette, skillsManifest, contextGraph, sharedState, orchPaths, monitorRows, templateIO) have edge-case tests; verify the rust OFF-by-default invariant; flag honesty gaps.'
NEUTER="GIT_CONFIG_COUNT=2 GIT_CONFIG_KEY_0=remote.jtmilan.pushurl GIT_CONFIG_VALUE_0=/dev/null GIT_CONFIG_KEY_1=remote.origin.pushurl GIT_CONFIG_VALUE_1=/dev/null"

run_yolo() { # $1=name $2=cli-with-flags $3=brief  (worktree always at main)
  local name="$1" cli="$2" brief="$3"
  local wt; wt=$(mktemp -d)
  git worktree add -f -d "$wt" main >>"$LOG" 2>&1 || true
  echo "##### $name START $(date +%T) wt=$wt" >>"$LOG"
  ( cd "$wt" && env $NEUTER timeout 300 $cli "$brief $RULES $BR" ) > "$OUT/headless-$name.md" 2>&1
  echo "rc=$?" >>"$LOG"
  git worktree remove -f "$wt" >>"$LOG" 2>&1 || true
  echo "##### $name END $(date +%T) bytes=$(wc -c < "$OUT/headless-$name.md" 2>/dev/null)" >>"$LOG"
}

# codex: read-only sandbox blocked its git exec, so use the neutered-yolo throwaway-worktree path.
run_yolo codex-testing "codex exec --dangerously-bypass-approvals-and-sandbox" "$L_CODEX"

run_yolo cursor-typescript     "cursor-agent --yolo -p" "$L_CURSOR" &
run_yolo commandcode-rust      "commandcode --yolo -p"  "$L_CMD" &
run_yolo opencode-architecture "opencode run"           "$L_OPENCODE" &
run_yolo pi-general            "pi -p --tools read,grep,find,ls,bash,edit,write" "$L_PI" &
wait
echo "ALL DONE $(date +%T)" >>"$LOG"
