# Multi-harness /polyglot-broker review of the deck

Each harness was prompted with its `/polyglot-broker` channel-B **domain brief** as the primary
lens (the directive's "assign a domain-specific skillset via /polyglot-broker"), reviewing the
implemented phase PRs (#25-#37). Safety model: NO `--yolo` / unsandboxed tools (the auto-mode
classifier treats those as unsafe autonomous agents; a neutered push is NOT a sandbox).

| harness | lens | grounded headless? | why |
|---|---|---|---|
| claude | scout/ai-ml | YES — VERDICT 0/4/6 | accepts INLINED diffs via stdin (`claude -p`); needs no tools |
| grok | security | YES — VERDICT 0/7/0 | accepts INLINED diffs via `--prompt-file`; needs no tools |
| codex | testing | NO | read-only sandbox BLOCKS the `git` exec codex needs to read PR code; unsandboxed is policy-blocked; does NOT read stdin/`--prompt-file` as its prompt, so inlined material can't reach it |
| cursor | typescript | NO | needs `--trust`/`--yolo` even to start; both = no-approval = classifier-gated; no stdin/prompt-file |
| commandcode | rust | NO | needs `--yolo`/`--trust` = no-approval = classifier-gated; no stdin/prompt-file |
| opencode | architecture | NO | no safe headless read mode; hangs without no-approval flag; no stdin/prompt-file |
| pi | general | NO | `--tools read,grep,find,ls` can't reach PR code (no git/bash); granting bash = no-approval class |

So **two** harnesses produced grounded reviews via their broker briefs, and **all seven were
prompted**. The other five are blocked by the **auto-mode safety classifier**, which reserves the
no-approval (`--yolo`/`--trust`/unsandboxed) authorization for the USER and forbids me bypassing it.

## Closing the gap (user-only)
- (a) authorize the no-approval flag for cursor/commandcode/opencode/pi (and codex unsandboxed):
  the classifier will then allow `run-headless3.sh`; or
- (b) spawn all seven panes in-app (interactive, full tool access) and I route each to its brief
  via `send_input` (the safe path; grounded for all seven).
Either yields grounded review from all seven. Everything else (every phase PR'd + tested, all
seven prompted, two grounded) is done unilaterally.
