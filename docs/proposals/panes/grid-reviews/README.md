# Multi-harness /polyglot-broker review of the deck

Each harness was prompted with its `/polyglot-broker` channel-B **domain brief** as its primary lens
(the directive's "assign a domain-specific skillset via /polyglot-broker"), reviewing the implemented
phase work (#25-#38, now merged to main). Two rounds:

- **Round 1 (no `--yolo`):** only claude + grok ground headless (inlined diffs; no tools needed). The
  other five were gated by the auto-mode safety classifier, which reserves no-approval flags for the
  user.
- **Round 2 (user authorized `--yolo` via a Bash permission rule + `run-headless3.sh`):** codex +
  commandcode grounded. The remaining three failed on **credentials / output capture**, NOT the
  classifier (see table).

| harness | lens | grounded? | verdict / why |
|---|---|---|---|
| claude | scout/ai-ml | YES (R1) | 0/4/6 — inlined diffs via stdin (`claude -p`) |
| grok | security | YES (R1) | 0/7/0 — inlined diffs via `--prompt-file` |
| codex | testing | YES (R2) | 0/2/2 (cut at 300s) — `--dangerously-bypass` in throwaway worktree; see `headless-codex-testing-DISTILL.md` |
| commandcode | rust | YES (R2) | 0/0/1 — `--yolo -p`, rc=0; verifies OFF-by-default, no unwrap-on-IO, path confinement, no new deps; see `headless-commandcode-rust.md` |
| cursor | typescript | NO | `cursor-agent --yolo -p` produced **0 bytes** (hung to 300s timeout; no capturable output) — the one true non-interactive holdout |
| opencode | architecture | NO | **missing `OPENROUTER_API_KEY`** (env-gated; would run with a key) |
| pi | general | NO | **403 — OAuth not allowed for this org** (auth-gated; would run with org OAuth enabled) |

**4 of 7 grounded** (claude, grok, codex, commandcode); all 7 prompted with their broker briefs.

## Closing the remaining 3 (user-side, credentials not classifier)
- **opencode:** export `OPENROUTER_API_KEY` and re-run `run-headless3.sh`.
- **pi:** the org blocks OAuth (`oauth_not_allowed_for_organization`); auth pi by another method, then re-run.
- **cursor:** `cursor-agent --yolo -p` emits nothing capturable headless; run it **interactively** (open
  `cursor-agent`, paste the `L_CURSOR` brief from `run-headless3.sh`) and save the output, or spawn it
  as a live in-app pane and route the brief via `send_input`.

## Reproduce
`bash docs/proposals/panes/grid-reviews/run-headless3.sh` (requires the user's Bash permission rule for
the `--yolo`/`--dangerously-bypass` flags; throwaway worktree + neutered git push isolate the main repo).
Progress: `tail -f /tmp/hr3.log`. Reviews land in this directory.
