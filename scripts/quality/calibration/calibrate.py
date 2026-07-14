#!/usr/bin/env python3
"""Reviewer self-test calibration checker (DESIGN §3.6 — the ponytail --selftest steal).

This script does NOT call any model. It is the deterministic CHECKER: given a
reviewer's JSON verdict for the known-BAD fixture and for the known-GOOD fixture,
it asserts the calibration invariant

    review(bad).has_block  AND  review(good).decision == APPROVE

  exit 0  -> reviewer is CALIBRATED (its real-PR APPROVE may be trusted this iter)
  exit 1  -> reviewer is UNTRUSTED  (fail-closed: coerce REQUEST_CHANGES + log)

"has_block" means the reviewer ranked the over-engineered + under-tested BAD
fixture strictly below the clean GOOD one by raising at least one blocking-class
finding (severity `block` or `major`) on it (equivalently decision==REQUEST_CHANGES,
since the contract ties them together). A model that rubber-stamps the BAD fixture,
or wrongly rejects the GOOD one, is treated as degraded and not trusted.

Inputs (two reviewer verdicts conforming to scripts/quality/calibration/review_prompt.md):
  --bad  PATH | -    reviewer JSON for the known_bad fixture (file or '-' for stdin)
  --good PATH | -    reviewer JSON for the known_good fixture (file or '-' for stdin)
At most one of the two may be '-' (stdin).

Each verdict is the strict-JSON object:
  {"decision": "APPROVE|REQUEST_CHANGES",
   "findings": [{"severity","domain","why","cite"}, ...],
   "most_important": <int|null>}

Exit codes: 0 calibrated, 1 untrusted, 2 usage/parse error.
"""

import argparse
import json
import sys

BLOCKING_SEVERITIES = {"block", "major"}
VALID_SEVERITIES = {"info", "minor", "major", "block"}
VALID_DECISIONS = {"APPROVE", "REQUEST_CHANGES"}


def _read_source(spec, stdin_used):
    """Read raw text from a file path, or stdin when spec == '-'."""
    if spec == "-":
        if stdin_used[0]:
            raise ValueError("stdin ('-') may only be used for one input")
        stdin_used[0] = True
        return sys.stdin.read()
    with open(spec, "r", encoding="utf-8") as fh:
        return fh.read()


def parse_verdict(raw, label):
    """Parse + minimally validate a reviewer verdict. Fail-closed on garbage."""
    try:
        v = json.loads(raw)
    except json.JSONDecodeError as e:
        raise ValueError("%s verdict is not valid JSON: %s" % (label, e))
    if not isinstance(v, dict):
        raise ValueError("%s verdict must be a JSON object" % label)

    # Accept either case: the Rust review gate emits lowercase (approve/request_changes). Normalize to
    # upper and write back so validation + downstream reads (decision_is_approve) are case-insensitive.
    decision = (v.get("decision") or "").upper()
    v["decision"] = decision
    if decision not in VALID_DECISIONS:
        raise ValueError(
            "%s verdict.decision must be one of %s, got %r"
            % (label, sorted(VALID_DECISIONS), decision)
        )

    findings = v.get("findings", [])
    if not isinstance(findings, list):
        raise ValueError("%s verdict.findings must be a list" % label)
    for i, f in enumerate(findings):
        if not isinstance(f, dict):
            raise ValueError("%s findings[%d] must be an object" % (label, i))
        sev = f.get("severity")
        if sev not in VALID_SEVERITIES:
            raise ValueError(
                "%s findings[%d].severity must be one of %s, got %r"
                % (label, i, sorted(VALID_SEVERITIES), sev)
            )
    return v


def has_block(verdict):
    """True iff the verdict carries >=1 blocking-class finding (block/major)."""
    return any(
        f.get("severity") in BLOCKING_SEVERITIES
        for f in verdict.get("findings", [])
    )


def decision_is_approve(verdict):
    return verdict.get("decision") == "APPROVE"


def calibrate(bad_verdict, good_verdict):
    """Return (ok: bool, reasons: list[str]) for the §3.6 invariant."""
    reasons = []
    bad_blocks = has_block(bad_verdict)
    good_approves = decision_is_approve(good_verdict)

    if bad_blocks:
        reasons.append("PASS  bad: has blocking finding (block/major) -> ranked below good")
    else:
        reasons.append(
            "FAIL  bad: NO blocking finding (decision=%s) -> rubber-stamped the "
            "over-engineered/under-tested fixture" % bad_verdict.get("decision")
        )

    if good_approves:
        reasons.append("PASS  good: decision == APPROVE")
    else:
        reasons.append(
            "FAIL  good: decision == %s (expected APPROVE) -> wrongly rejected the "
            "clean fixture" % good_verdict.get("decision")
        )

    return (bad_blocks and good_approves), reasons


def main(argv=None):
    p = argparse.ArgumentParser(
        prog="calibrate.py",
        description="Reviewer self-test calibration checker (§3.6). "
        "Does NOT call a model; checks two pre-computed reviewer verdicts.",
    )
    p.add_argument("--bad", required=True, metavar="PATH|-",
                   help="reviewer JSON verdict for the known_bad fixture")
    p.add_argument("--good", required=True, metavar="PATH|-",
                   help="reviewer JSON verdict for the known_good fixture")
    p.add_argument("--quiet", action="store_true", help="suppress per-check lines")
    args = p.parse_args(argv)

    stdin_used = [False]
    try:
        bad_raw = _read_source(args.bad, stdin_used)
        good_raw = _read_source(args.good, stdin_used)
        bad_v = parse_verdict(bad_raw, "bad")
        good_v = parse_verdict(good_raw, "good")
    except (OSError, ValueError) as e:
        print("calibrate: error: %s" % e, file=sys.stderr)
        return 2

    ok, reasons = calibrate(bad_v, good_v)
    if not args.quiet:
        for r in reasons:
            print(r, file=sys.stderr)
    if ok:
        print("CALIBRATED — reviewer trusted this iteration", file=sys.stderr)
        return 0
    print("UNTRUSTED — fail-closed: coerce REQUEST_CHANGES + log", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
