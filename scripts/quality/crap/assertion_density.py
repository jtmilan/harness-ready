#!/usr/bin/env python3
"""
assertion_density.py — cheap deterministic ANTI-GAMING floor (DESIGN §3.10).

The hole CRAP-arithmetic can't plug: coverage rewards EXECUTION, not ASSERTION.
An autonomous fix worker can drop a method's CRAP by adding assertion-FREE tests
that merely *call* the method — coverage rises, CRAP falls, no real verification.

This check is the "cheap deterministic assertion-density floor" guardrail:

  given a unified diff, find the NEWLY-ADDED test functions, count the assertions
  inside each, and FLAG when covered lines rose but per-new-test assertion count
  is below a floor.

It is grep/AST-lite by design (regex over added (+) lines of the diff) — no test
runner, no compiler, no LLM. Report-only: it emits JSON; it does not block.

Recognizes the assertion idioms of both repo languages:
  Rust : assert!, assert_eq!, assert_ne!, assert_matches!, debug_assert*!,
         #[should_panic], .unwrap()/.expect() in a #[test] (weak), panic!,
         insta::assert_*!, claim::assert_*
  JS/TS: assert(...), assert.<x>(...), expect(...).<matcher>, .toBe/.toEqual/...,
         chai should/expect, node:test t.assert*, vitest/jest expect

A new test function with assertions BELOW the floor is "thin"; if any thin test
exists AND covered_lines_delta > 0 (coverage rose) we set gamed_suspected=true.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional


DEFAULT_FLOOR = 1   # at least one real assertion per new test


# Added-line assertion idioms. Each pattern matches when the construct appears
# on a `+` line of the diff.
RUST_ASSERT = re.compile(
    r"\b("
    r"assert(_eq|_ne|_matches)?!"        # assert! / assert_eq! / assert_ne! / assert_matches!
    r"|debug_assert(_eq|_ne)?!"
    r"|panic!"
    r"|insta::assert\w*!"
    r"|claim::assert\w+"
    r")"                                  # no trailing \b: `!` is non-word, never borders a word
)
RUST_WEAK = re.compile(r"\.(unwrap|expect)\s*\(")          # weak signal, counts at half
RUST_SHOULD_PANIC = re.compile(r"#\[should_panic")

JS_ASSERT = re.compile(
    r"("
    r"\bassert\s*\("
    r"|\bassert\.\w+\s*\("                  # node assert.strictEqual(...)
    r"|\bexpect\s*\(.*\)\s*\.\s*\w+"        # expect(x).toBe(...)  (.* tolerates nested parens)
    r"|\.should\b"                            # chai should
    r"|\bt\.assert\w*\s*\("                  # node:test t.assert*
    r")"
)

# Heuristics for "this added line starts a test function".
RUST_TEST_ATTR = re.compile(r"#\[(test|tokio::test|rstest)")
RUST_FN = re.compile(r"\bfn\s+(\w+)")
JS_TEST_DECL = re.compile(
    r"\b(test|it)\s*\(\s*['\"`]([^'\"`]+)['\"`]"          # test('name', ...) / it('name', ...)
    r"|\bfunction\s+(test\w+)\s*\("                        # function testFoo(
)


@dataclass
class TestUnit:
    file: str
    name: str
    lang: str
    assertions: float = 0.0      # weak idioms count 0.5
    weak_only: bool = False
    body_added_lines: int = 0


def _added_lines(diff_text: str):
    """Yield (current_file, line_without_plus) for every added (+) content line."""
    cur = None
    for raw in diff_text.splitlines():
        if raw.startswith("+++ "):
            # +++ b/path/to/file
            p = raw[4:].strip()
            if p.startswith("b/"):
                p = p[2:]
            cur = p
            continue
        if raw.startswith("diff --git"):
            cur = None
            continue
        if raw.startswith("+") and not raw.startswith("+++"):
            yield cur, raw[1:]


def parse_tests(diff_text: str) -> list[TestUnit]:
    """Walk added lines, segment into test functions, count assertions."""
    units: list[TestUnit] = []
    cur: Optional[TestUnit] = None
    pending_rust_test = False   # saw #[test] on prior added line
    pending_should_panic = False

    for file, line in _added_lines(diff_text):
        if file is None:
            continue
        is_rust = file.endswith(".rs")
        is_js = file.endswith((".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs"))

        if is_rust:
            if RUST_SHOULD_PANIC.search(line):
                pending_should_panic = True
            if RUST_TEST_ATTR.search(line):
                pending_rust_test = True
                continue
            m = RUST_FN.search(line)
            if m and pending_rust_test:
                cur = TestUnit(file=file, name=m.group(1), lang="rust")
                # #[should_panic] is itself an assertion of failure
                if pending_should_panic:
                    cur.assertions += 1.0
                units.append(cur)
                pending_rust_test = False
                pending_should_panic = False
                continue
            if cur is not None and cur.file == file:
                cur.body_added_lines += 1
                if RUST_ASSERT.search(line):
                    cur.assertions += 1.0
                elif RUST_WEAK.search(line):
                    cur.assertions += 0.5

        elif is_js:
            tm = JS_TEST_DECL.search(line)
            if tm:
                name = tm.group(2) or tm.group(3) or "<anon>"
                cur = TestUnit(file=file, name=name, lang="js")
                units.append(cur)
                # an assertion may also be on the same line
                if JS_ASSERT.search(line):
                    cur.assertions += 1.0
                continue
            if cur is not None and cur.file == file:
                cur.body_added_lines += 1
                if JS_ASSERT.search(line):
                    cur.assertions += 1.0

    for u in units:
        u.weak_only = (u.assertions > 0 and u.assertions < 1.0)
    return units


def build_report(units: list[TestUnit], floor: int,
                 covered_lines_delta: int) -> dict:
    thin = [u for u in units if u.assertions < floor]
    coverage_rose = covered_lines_delta > 0

    # The load-bearing signal: coverage went UP while new tests are assertion-thin
    # => likely coverage-padding to game CRAP.
    gamed_suspected = coverage_rose and len(thin) > 0

    def u_dict(u: TestUnit) -> dict:
        return {
            "file": u.file, "name": u.name, "lang": u.lang,
            "assertions": u.assertions, "weak_only": u.weak_only,
            "body_added_lines": u.body_added_lines,
            "below_floor": u.assertions < floor,
        }

    return {
        "floor": floor,
        "new_tests_found": len(units),
        "thin_tests": [u_dict(u) for u in thin],
        "all_tests": [u_dict(u) for u in units],
        "covered_lines_delta": covered_lines_delta,
        "coverage_rose": coverage_rose,
        "gamed_suspected": gamed_suspected,
        "reason": _reason(units, thin, coverage_rose),
        "report_only": True,
    }


def _reason(units, thin, coverage_rose) -> str:
    if not units:
        return "no new tests in diff"
    if not thin:
        return f"{len(units)} new test(s), all meet the assertion floor"
    names = ", ".join(u.name for u in thin[:5])
    suffix = "" if coverage_rose else " (coverage did NOT rise — lower concern)"
    return (f"{len(thin)}/{len(units)} new test(s) below assertion floor: "
            f"{names}{suffix}")


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description="Assertion-density anti-gaming floor (§3.10)")
    ap.add_argument("--diff", type=Path,
                    help="unified diff file (default: read stdin)")
    ap.add_argument("--floor", type=int, default=DEFAULT_FLOOR,
                    help="minimum assertions per new test (default 1)")
    ap.add_argument("--covered-lines-delta", type=int, default=0,
                    help="net change in covered lines vs base (>0 => coverage rose)")
    ap.add_argument("--out", type=Path)
    args = ap.parse_args(argv)

    if args.diff:
        diff_text = args.diff.read_text()
    else:
        diff_text = sys.stdin.read()

    units = parse_tests(diff_text)
    report = build_report(units, args.floor, args.covered_lines_delta)

    text = json.dumps(report, indent=2)
    print(text)
    if args.out:
        args.out.write_text(text + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
