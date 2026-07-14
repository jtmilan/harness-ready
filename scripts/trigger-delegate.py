#!/usr/bin/env python3
"""trigger-delegate.py — fire a Phase-16 `team_delegate` at the running app, for live-verify.

Dials the app's Unix-domain mutation socket directly (the same seam the operator MCP's
team_delegate tool uses) and sends ONE `Delegate` SocketRequest line, then prints the
fast-ack reply. This is the rawest Tier-2 trigger — no operator MCP wiring needed; the app
must be running and same-user (euid-gated).

The app only ACTS on this if all three gates are open:
  1. delegate-live cargo feature  (the installed app was built with it)
  2. mcp-config.json allow_mutations: true
  3. mcp-config.json autonomy_ceiling >= 1   <-- the one you must flip for a test
Otherwise the reply is a refusal code (MUTATIONS_DISABLED / AUTONOMY_DISABLED / etc.).

SAFETY: point this at a parent pane that lives in a THROWAWAY repo. Workers spawn worktrees
off that pane's repo and run cargo/git/edits there (cargo = arbitrary code execution — see
.paul/analysis/worker-capability-model.md). NEVER target a pane on a repo you care about.

Usage:
  python3 scripts/trigger-delegate.py --parent <pane-id> --goal "..." [--workers 2]
  python3 scripts/trigger-delegate.py --list        # show live panes (pick a parent id)
"""
import argparse
import json
import os
import socket
import sys

SUPPORT = os.path.expanduser("~/Library/Application Support")
SOCK = os.path.join(SUPPORT, "agent-teams-mcp.sock")
LIVE = os.path.join(SUPPORT, "agent-teams-live.json")
CFG = os.path.join(SUPPORT, "mcp-config.json")


def show_context():
    try:
        cfg = json.load(open(CFG))
        print(f"gates: allow_mutations={cfg.get('allow_mutations')} "
              f"autonomy_ceiling={cfg.get('autonomy_ceiling')} "
              f"{'(ARMED)' if cfg.get('allow_mutations') and cfg.get('autonomy_ceiling', 0) >= 1 else '(NOT armed — autonomy_ceiling must be >=1)'}")
    except Exception as e:
        print(f"gates: could not read {CFG}: {e}")
    try:
        live = json.load(open(LIVE))
        print(f"app_pid={live.get('app_pid')}  live panes:")
        for w in live.get("workspaces", []):
            print(f"  {w['id']:16} {w.get('harness',''):8} {w.get('repo','')}")
    except Exception as e:
        print(f"live panes: could not read {LIVE}: {e}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--parent", help="parent pane id (must be a LIVE pane in a THROWAWAY repo)")
    ap.add_argument("--goal", help="the delegation goal")
    ap.add_argument("--workers", type=int, default=2)
    ap.add_argument("--depth", type=int, default=1)
    ap.add_argument("--list", action="store_true", help="just show gates + live panes")
    args = ap.parse_args()

    show_context()
    if args.list:
        return 0
    if not args.parent or not args.goal:
        print("\nERROR: --parent and --goal are required (or use --list). "
              "Pick a parent pane in a THROWAWAY repo from the list above.", file=sys.stderr)
        return 2

    if not os.path.exists(SOCK):
        print(f"ERROR: socket not found at {SOCK} — is the app running?", file=sys.stderr)
        return 2

    req = {"op": "delegate", "parent_id": args.parent, "goal": args.goal,
           "max_workers": args.workers, "depth": args.depth}
    line = json.dumps(req) + "\n"
    print(f"\n>>> dialing {SOCK}\n>>> {line.strip()}")
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(10)
    try:
        s.connect(SOCK)
        s.sendall(line.encode())
        buf = b""
        while not buf.endswith(b"\n"):
            chunk = s.recv(4096)
            if not chunk:
                break
            buf += chunk
    finally:
        s.close()
    reply = buf.decode(errors="replace").strip()
    print(f"<<< {reply}")
    try:
        r = json.loads(reply)
        if r.get("ok"):
            data = r.get("data", {})
            print(f"\nACCEPTED: run_id={data.get('run_id')} workers={data.get('workers')}")
            print("The detached controller is now running. Watch the PARENT pane in the app for a")
            print("one-line digest when it finishes, and the <repo>/bridge/<run_id>/ dir for reports.")
        else:
            print(f"\nREFUSED: code={r.get('code')} detail={r.get('detail')}")
            if r.get("code") == "AUTONOMY_DISABLED":
                print("→ flip autonomy_ceiling to 1 in mcp-config.json and retry.")
    except Exception:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
