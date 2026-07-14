# Standalone CRAP delta runner (DESIGN §3.10) — REPORT-ONLY

Cheap, deterministic, language-agnostic Change-Risk-Anti-Patterns signal for the
loop's quality gate. **It does not gate, block, push, or touch `lib.rs`** — it
only emits a JSON verdict the loop driver can read. Gate wiring is later work.

`CRAP(m) = CC(m)² × (1 − Cov(m))³ + CC(m)`, threshold **30**.

## Tools (same formula + threshold both sides → one gate logic)

| Half | Tool | Coverage input |
|---|---|---|
| Rust core | `cargo-crap` (minikin/cargo-crap) | `cargo llvm-cov --lcov` lcov file |
| JS / TS | `fallow health` (npm `@fallow-cli`) | Istanbul / c8 `coverage-final.json` |

`fallow` is Rust-built but analyzes **JS/TS only** — it does NOT read Rust, so
`cargo-crap` guards the risky Rust half. Gating fallow alone = Rust CRAP-blind.

## Files

- `crap_delta.py` — the runner. Drives both tools, normalizes to one schema,
  computes hotspots / delta / new-over-threshold, emits the §3.10 verdict.
- `assertion_density.py` — the anti-gaming floor. Given a diff, flags
  assertion-light new tests when coverage rose (coverage-padding to game CRAP).
- `run-crap-delta.sh` — driver/wrapper: resolves abs paths, checks tools, runs
  the verdict. Report-only, exits 0 regardless of verdict.
- `selftest.sh` — proves the pipeline end-to-end on a tiny Rust + JS sample
  (does NOT build the full app). `bash selftest.sh` → "ALL CHECKS PASSED".
- `sample/` — tiny fixtures (one trivial fn + one gnarly uncovered fn per lang)
  + gamed/clean diff fixtures for the assertion floor.

## Verdict shape

```json
{
  "threshold": 30.0,
  "languages": ["js", "rust"],
  "hotspots":           [ /* top-N methods by absolute CRAP */ ],
  "delta":              [ /* touched methods whose CRAP ROSE vs --base */ ],
  "new_over_threshold": [ /* NEW methods with CRAP > 30 */ ],
  "gate_would_block":   true,   /* advisory: would a delta gate veto this merge */
  "gate_reason":        "...",
  "report_only":        true
}
```

## Usage

```bash
# Rust: capture a BASE baseline on the captured origin/main checkout first.
cargo crap --lcov base.lcov --format json --output base-crap.json

# Then run the verdict for HEAD vs that captured base:
./run-crap-delta.sh \
  --rust-lcov head.lcov --rust-project core --rust-baseline base-crap.json \
  --js-root app --js-coverage app/coverage/coverage-final.json \
  --js-base-coverage base/coverage/coverage-final.json \
  --threshold 30 --top 10

# Anti-gaming floor on a diff:
git diff origin/main... | ./assertion_density.py --floor 1 --covered-lines-delta 18
```

## Design invariants honored

- **STRICTER-only veto.** `gate_would_block` flips on regressions / new-over-threshold;
  it is never a pass signal and can never forge a merge (same monotonicity as
  `critique_verdict_downgrade`).
- **Delta-scoped, never absolute.** Diffs against a CAPTURED base ref (cargo-crap
  baseline json / a base coverage file), never live local main (fold mutates main).
  Pre-existing whole-repo debt the loop did not author never blocks it.
- **Deterministic, LLM-free, dependency-light.** Pure arithmetic over coverage
  artifacts; stdlib-only Python; no extra runtime deps beyond the two CLIs.
- **Anti-gaming is load-bearing.** CRAP is never a worker success target; the
  assertion-density floor catches assertion-free coverage-padding for free.

## Tool versions verified (2026-06-20)

- cargo-crap 0.2.2 · cargo-llvm-cov 0.8.7 · fallow 2.101.0
- Homebrew rust 1.95.0 (no rustup): llvm-cov needs `llvm-profdata`/`llvm-cov`
  from `/opt/homebrew/opt/llvm/bin` — `selftest.sh` sets them via env.
