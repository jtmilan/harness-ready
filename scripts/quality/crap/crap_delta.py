#!/usr/bin/env python3
"""
crap_delta.py — standalone, REPORT-ONLY CRAP delta runner (DESIGN §3.10).

Computes the Change Risk Anti-Patterns metric  CRAP = CC^2 * (1 - Cov)^3 + CC
(threshold 30) for a changeset, in BOTH languages of the agent-teams repo:

  * Rust core  -> cargo-crap (minikin/cargo-crap), reads a `cargo llvm-cov --lcov` file.
  * JS / TS    -> fallow (npm @fallow-cli), reads Istanbul/c8 coverage-final.json.

Same formula + same threshold both sides => ONE gate logic across languages.

It is a PURE, DETERMINISTIC, STRICTER-ONLY signal. It NEVER runs tests and it
NEVER gates/blocks anything itself — it only emits a JSON verdict:

    {
      "hotspots":            [ top-N methods by absolute CRAP ],
      "delta":               [ methods whose CRAP ROSE vs the BASE ref ],
      "new_over_threshold":  [ NEW methods with CRAP > threshold ],
      "gate_would_block":    bool,     # advisory: would a delta gate veto this merge?
      ...
    }

The `--base` ref is a CAPTURED base (a saved baseline artifact), NOT live main —
fold transiently mutates main, so the caller must capture origin/main first
(the established gotcha, DESIGN §3.10-3).

Wiring into lib.rs (gating) is deliberately OUT OF SCOPE here. This is the cheap
deterministic component; the loop driver decides what to do with the verdict.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from dataclasses import dataclass, asdict, field
from pathlib import Path
from typing import Optional


DEFAULT_THRESHOLD = 30.0
# A CRAP delta below this is noise (matches cargo-crap's default epsilon).
DEFAULT_EPSILON = 0.01


@dataclass
class Method:
    """A single function/method's CRAP record, language-agnostic."""
    lang: str            # "rust" | "js"
    file: str
    function: str
    line: int
    cc: float            # cyclomatic complexity
    coverage: float      # 0..100 (percent)
    crap: float

    def key(self) -> str:
        # Identity for delta matching. line is deliberately excluded (it drifts
        # as code above shifts); file+function is the stable identity.
        return f"{self.lang}:{self.file}:{self.function}"


@dataclass
class DeltaEntry:
    method: Method
    baseline_crap: Optional[float]
    delta: float
    status: str          # "regressed" | "new" | "improved" | "unchanged" | "removed"


# --------------------------------------------------------------------------
# CRAP arithmetic (used for the JS delta + as a cross-check)
# --------------------------------------------------------------------------

def crap_score(cc: float, coverage_pct: float) -> float:
    cov = max(0.0, min(1.0, coverage_pct / 100.0))
    return cc * cc * (1.0 - cov) ** 3 + cc


# --------------------------------------------------------------------------
# Rust side — cargo-crap
# --------------------------------------------------------------------------

def run_cargo_crap(lcov: Path, project: Path, threshold: float,
                   baseline: Optional[Path]) -> tuple[list[Method], dict]:
    """Run cargo-crap --format json. Returns (head methods, raw delta json|{})."""
    base_cmd = [
        "cargo", "crap",
        "--lcov", str(lcov),
        "--path", str(project),
        "--threshold", str(threshold),
        "--format", "json",
    ]
    raw_delta: dict = {}
    if baseline is not None:
        cmd = base_cmd + ["--baseline", str(baseline)]
        out = _run_json(cmd, "cargo-crap (delta)")
        raw_delta = out
        entries = out.get("entries", [])
    else:
        out = _run_json(base_cmd, "cargo-crap")
        entries = out.get("entries", [])

    methods = [
        Method(
            lang="rust",
            file=e["file"],
            function=e["function"],
            line=int(e.get("line", 0)),
            cc=float(e.get("cyclomatic", 0.0)),
            coverage=float(e.get("coverage", 0.0)),
            crap=float(e.get("crap", 0.0)),
        )
        for e in entries
    ]
    return methods, raw_delta


def save_cargo_crap_baseline(lcov: Path, project: Path, threshold: float,
                             out_path: Path) -> None:
    cmd = [
        "cargo", "crap",
        "--lcov", str(lcov),
        "--path", str(project),
        "--threshold", str(threshold),
        "--format", "json",
        "--output", str(out_path),
    ]
    subprocess.run(cmd, check=True)


# --------------------------------------------------------------------------
# JS side — fallow health
# --------------------------------------------------------------------------

def run_fallow_health(root: Path, coverage: Path, threshold: float) -> list[Method]:
    """Run fallow health -f json with Istanbul coverage. Returns methods.

    fallow only emits findings ABOVE its complexity/crap floor, so to get a full
    per-function picture we lower --max-crap is NOT enough (it still filters by
    complexity). We therefore take whatever findings carry a `crap` field; that
    is sufficient for hotspots + the over-threshold gate (the only methods that
    can trip the gate are exactly the ones fallow surfaces)."""
    cmd = [
        "fallow", "health",
        "--root", str(root),
        "--coverage", str(coverage),
        "--max-crap", str(threshold),
        "-f", "json",
    ]
    out = _run_json(cmd, "fallow health")
    methods: list[Method] = []
    for f in out.get("findings", []):
        if "crap" not in f or f.get("crap") is None:
            continue
        methods.append(Method(
            lang="js",
            file=f.get("path", "?"),
            function=f.get("name", "?"),
            line=int(f.get("line", 0)),
            cc=float(f.get("cyclomatic", 0.0)),
            coverage=float(f.get("coverage_pct", 0.0)),
            crap=float(f.get("crap", 0.0)),
        ))
    return methods


# --------------------------------------------------------------------------
# Delta computation
# --------------------------------------------------------------------------

def delta_from_cargo_crap(raw_delta: dict, epsilon: float) -> list[DeltaEntry]:
    """Use cargo-crap's NATIVE delta output (status/baseline_crap/delta)."""
    entries: list[DeltaEntry] = []
    for e in raw_delta.get("entries", []):
        m = Method(
            lang="rust",
            file=e["file"],
            function=e["function"],
            line=int(e.get("line", 0)),
            cc=float(e.get("cyclomatic", 0.0)),
            coverage=float(e.get("coverage", 0.0)),
            crap=float(e.get("crap", 0.0)),
        )
        status = e.get("status", "unchanged")
        bc = e.get("baseline_crap")
        d = e.get("delta")
        if d is None:
            d = m.crap - (bc if bc is not None else 0.0)
        # Normalize cargo-crap's status to ours; treat tiny deltas as unchanged.
        if status not in ("new", "removed") and abs(d) <= epsilon:
            status = "unchanged"
        entries.append(DeltaEntry(method=m, baseline_crap=bc, delta=float(d), status=status))
    # removed entries (present in baseline, gone now) — keep for completeness
    for e in raw_delta.get("removed", []):
        m = Method(lang="rust", file=e.get("file", "?"), function=e.get("function", "?"),
                   line=int(e.get("line", 0)), cc=float(e.get("cyclomatic", 0.0)),
                   coverage=float(e.get("coverage", 0.0)), crap=float(e.get("crap", 0.0)))
        entries.append(DeltaEntry(method=m, baseline_crap=e.get("crap"), delta=0.0,
                                  status="removed"))
    return entries


def delta_by_diffing(head: list[Method], base: list[Method],
                     epsilon: float) -> list[DeltaEntry]:
    """Compute a delta by joining head vs base on file+function key.

    Used for JS (fallow has no native per-function baseline diff) and as the
    Rust path when no native baseline json is available."""
    base_by_key = {m.key(): m for m in base}
    head_keys = set()
    entries: list[DeltaEntry] = []
    for m in head:
        head_keys.add(m.key())
        b = base_by_key.get(m.key())
        if b is None:
            entries.append(DeltaEntry(method=m, baseline_crap=None,
                                      delta=m.crap, status="new"))
            continue
        d = m.crap - b.crap
        if abs(d) <= epsilon:
            status = "unchanged"
        elif d > 0:
            status = "regressed"
        else:
            status = "improved"
        entries.append(DeltaEntry(method=m, baseline_crap=b.crap, delta=d, status=status))
    # removed
    for b in base:
        if b.key() not in head_keys:
            entries.append(DeltaEntry(method=b, baseline_crap=b.crap, delta=0.0,
                                      status="removed"))
    return entries


# --------------------------------------------------------------------------
# Verdict assembly
# --------------------------------------------------------------------------

def build_verdict(methods: list[Method], deltas: list[DeltaEntry],
                  threshold: float, top_n: int) -> dict:
    hotspots = sorted(methods, key=lambda m: m.crap, reverse=True)[:top_n]

    regressions = [d for d in deltas if d.status == "regressed"]
    regressions.sort(key=lambda d: d.delta, reverse=True)

    # New methods over the threshold are the hard "added debt" signal.
    new_over = [
        d for d in deltas
        if d.status == "new" and d.method.crap > threshold
    ]
    new_over.sort(key=lambda d: d.method.crap, reverse=True)

    # DELTA gate (advisory): block iff a touched method's CRAP ROSE above baseline,
    # OR a new method exceeds the threshold. STRICTER-only — never a pass signal.
    gate_would_block = bool(regressions) or bool(new_over)

    def m_dict(m: Method) -> dict:
        return {
            "lang": m.lang, "file": m.file, "function": m.function,
            "line": m.line, "cc": round(m.cc, 2),
            "coverage": round(m.coverage, 2), "crap": round(m.crap, 2),
            "over_threshold": m.crap > threshold,
        }

    def d_dict(d: DeltaEntry) -> dict:
        out = m_dict(d.method)
        out["baseline_crap"] = (round(d.baseline_crap, 2)
                                if d.baseline_crap is not None else None)
        out["delta"] = round(d.delta, 2)
        out["status"] = d.status
        return out

    return {
        "threshold": threshold,
        "languages": sorted({m.lang for m in methods}),
        "method_count": len(methods),
        "hotspots": [m_dict(m) for m in hotspots],
        "delta": [d_dict(d) for d in regressions],
        "new_over_threshold": [d_dict(d) for d in new_over],
        "gate_would_block": gate_would_block,
        "gate_reason": _gate_reason(regressions, new_over),
        "report_only": True,
    }


def _gate_reason(regressions: list[DeltaEntry], new_over: list[DeltaEntry]) -> str:
    parts = []
    if regressions:
        worst = regressions[0]
        parts.append(
            f"{len(regressions)} touched method(s) raised CRAP "
            f"(worst: {worst.method.function} +{worst.delta:.1f} -> {worst.method.crap:.1f})"
        )
    if new_over:
        worst = new_over[0]
        parts.append(
            f"{len(new_over)} new method(s) over threshold "
            f"(worst: {worst.method.function} CRAP {worst.method.crap:.1f})"
        )
    return "; ".join(parts) if parts else "clean — no CRAP regression on touched methods"


# --------------------------------------------------------------------------
# subprocess helper
# --------------------------------------------------------------------------

def _run_json(cmd: list[str], label: str) -> dict:
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode != 0 and not proc.stdout.strip():
        sys.stderr.write(f"[{label}] exit {proc.returncode}\n{proc.stderr}\n")
        raise SystemExit(2)
    # Some tools emit warnings on stdout before the JSON; find the first '{'.
    text = proc.stdout
    brace = text.find("{")
    if brace < 0:
        sys.stderr.write(f"[{label}] no JSON in stdout:\n{text[:500]}\n{proc.stderr[:500]}\n")
        raise SystemExit(2)
    try:
        return json.loads(text[brace:])
    except json.JSONDecodeError as ex:
        sys.stderr.write(f"[{label}] bad JSON: {ex}\n{text[brace:brace+500]}\n")
        raise SystemExit(2)


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------

def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description="Standalone report-only CRAP delta runner (§3.10)")
    ap.add_argument("--threshold", type=float, default=DEFAULT_THRESHOLD)
    ap.add_argument("--epsilon", type=float, default=DEFAULT_EPSILON)
    ap.add_argument("--top", type=int, default=10, help="hotspots: top-N by CRAP")

    # Rust inputs
    ap.add_argument("--rust-lcov", type=Path, help="head lcov (cargo llvm-cov --lcov)")
    ap.add_argument("--rust-project", type=Path, default=Path("."),
                    help="Rust crate/workspace root for cargo-crap --path")
    ap.add_argument("--rust-baseline", type=Path,
                    help="captured cargo-crap baseline json (the --base ref)")

    # JS inputs
    ap.add_argument("--js-root", type=Path, help="JS project root for fallow")
    ap.add_argument("--js-coverage", type=Path,
                    help="head Istanbul coverage-final.json")
    ap.add_argument("--js-base-coverage", type=Path,
                    help="captured BASE Istanbul coverage-final.json (the --base ref)")
    ap.add_argument("--js-base-root", type=Path,
                    help="checked-out BASE source root for fallow (defaults to --js-root)")

    ap.add_argument("--out", type=Path, help="write verdict JSON to file (also stdout)")
    args = ap.parse_args(argv)

    all_methods: list[Method] = []
    all_deltas: list[DeltaEntry] = []

    # ---- Rust ----
    if args.rust_lcov:
        methods, raw_delta = run_cargo_crap(
            args.rust_lcov, args.rust_project, args.threshold, args.rust_baseline)
        all_methods += methods
        if args.rust_baseline:
            all_deltas += delta_from_cargo_crap(raw_delta, args.epsilon)

    # ---- JS ----
    if args.js_coverage:
        js_root = args.js_root or Path(".")
        head_js = run_fallow_health(js_root, args.js_coverage, args.threshold)
        all_methods += head_js
        if args.js_base_coverage:
            base_root = args.js_base_root or js_root
            base_js = run_fallow_health(base_root, args.js_base_coverage, args.threshold)
            all_deltas += delta_by_diffing(head_js, base_js, args.epsilon)

    if not args.rust_lcov and not args.js_coverage:
        ap.error("provide at least one of --rust-lcov / --js-coverage")

    verdict = build_verdict(all_methods, all_deltas, args.threshold, args.top)

    text = json.dumps(verdict, indent=2)
    print(text)
    if args.out:
        args.out.write_text(text + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
