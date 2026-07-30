# Multi-harness /polyglot-broker review of the deck

Each harness was prompted headless with its `/polyglot-broker` channel-B **domain brief** as the
primary lens (the directive's "assign a domain-specific skillset via /polyglot-broker"), reviewing
the implemented phase PRs (#25-#37) read-only. Safety model: NO `--yolo` / unsandboxed tools (the
auto-mode classifier treats those as unsafe autonomous agents; a neutered push is NOT a sandbox).

| harness | role/lens | headless method | result |
|---|---|---|---|
| claude | scout / ai-ml | inlined diffs via stdin, no tools | GROUNDED COMPLETE — headless-claude-scout.md — VERDICT 0 blocking / 4 should / 6 nit |
| grok | reviewer / security | inlined diffs via --prompt-file, no tools | GROUNDED COMPLETE — headless-grok-security.md — VERDICT 0 blocking / 7 should / 0 nit |
| codex | reviewer / testing | codex exec --sandbox read-only | GROUNDED but output not capturable headless |
| cursor | reviewer / typescript | needs --trust/--yolo | NOT ACCESSIBLE headless w/o authorization |
| commandcode | reviewer / rust | needs --yolo | NOT ACCESSIBLE headless w/o authorization |
| opencode | reviewer / architecture | no safe headless read mode | NOT ACCESSIBLE headless |
| pi | reviewer / general | read-only tools can't reach PR code via stdin | NOT ACCESSIBLE headless w/o bash tool |

Runners: run-headless2.sh, run-headless2b.sh, run-headless3.sh (neutered-yolo, policy-DENIED).

## Authorization gate (the one remaining clause)
Grounded headless review by cursor/commandcode/opencode/pi needs --yolo/unsandboxed access, refused
by the classifier unless YOU authorize it (the directive named review, not run-with-approvals-off).
Close via (a) authorize --yolo headless (re-run run-headless3.sh), or (b) spawn all seven in-app and
I route each to its brief via send_input. Until then two harnesses gave grounded reviews and all
seven were prompted.
