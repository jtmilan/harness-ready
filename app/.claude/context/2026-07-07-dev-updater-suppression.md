# Context Brief — dev-app Update-card suppression + flavor-guarded self-updater

**Date:** 2026-07-07 · **Intent:** bugfix · **Trigger:** operator: Dev app shows "Update Available" after every prod rebuild.

## Topic / Intent

D34 self-updater = mtime compare (self binary vs dev-source-pointer bundle) + `apply_update` ditto helper. Three defects found reading the code live:
1. Both apps' dev-source pointers name the SAME build slot (`target/release/bundle/macos/Agent Teams.app`) — either flavor's rebuild makes the OTHER app announce an update forever (edge-triggered per mtime, re-fires each build).
2. `apply_update` hardcodes dest `/Applications/Agent Teams.app` — invoked from the DEV app it would overwrite PROD (no rebrand, wrong dest).
3. No flavor awareness: prod (default build) can be offered/applied a delegate-live bundle and vice versa.

## Best Practices Found

- EXTRACTED (memory `dev-updater-and-app-decouple`): run the /Applications copy; install-app.sh swaps + seeds the dev-source pointer; Update Now card = D34.
- EXTRACTED (lib.rs:5874-5930): apply_update helper is &&-chained with stable-cert re-sign (TCC survival) — keep untouched semantics for prod.
- EXTRACTED (lib.rs:557,13666): `update_available(self_mtime, target_mtime)` pure + edge-trigger `announced` — extend, don't rewrite.
- EXTRACTED (install-app.sh): --testing rebrands bundle id + LSEnvironment AFTER ditto, BEFORE codesign; dev app is exclusively installed by the script (which relaunches it) — an in-app updater in dev has no correct behavior available.

## Industry-Standard Architecture / Patterns

- INFERRED: updater eligibility = (running from the exact install the updater knows how to replace) AND (target artifact flavor == self flavor) AND (strictly newer). Standard self-update hygiene: never offer an artifact you can't correctly apply.

## Anti-Patterns

- REJECTED: separate build slots per flavor (restructures target dir + script; bigger surface than the guard).
- REJECTED: rebrand-aware apply in dev (script already owns dev installs; duplicating the rebrand in Rust = drift risk).
- REJECTED: version-string compare (rebuilds share v0.9.0; mtime stays the signal).

## Open Questions

- AMBIGUOUS: none material; marker file name `at-flavor` in Contents/Resources chosen (survives ditto, codesign --deep covers it).

## Sources

lib.rs update tick @13662-13686, apply_update @5879-5930, update_available @557, default_dev_source_path @564; scripts/install-app.sh; memory `dev-updater-and-app-decouple`; operator screenshot (Update card in Dev).

**Conformance:** conforms — eligibility guard (install-path check + flavor marker equality) in front of both announce and apply; prod behavior byte-identical when markers match; dev card structurally silenced. Deviation: none.
