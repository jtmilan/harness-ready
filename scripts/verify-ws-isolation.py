#!/usr/bin/env python3
"""Live end-to-end verify of the Phase-2 workspace-isolation partition.

One command: builds the real `agent-teams-mcp` sidecar (memory + task tools),
then drives it over stdio MCP as four identities against ONE shared temp state
dir (the file store persists across sessions):

  1. wsA (ws76101x0)  — creates a note + a task (stamped with its workspace)
  2. wsB (ws88888x1)  — sharing OFF: list hides wsA; by-id reads → CROSS_WORKSPACE
  3. wsB again        — sharing ON (registry opts wsA in): wsA note becomes visible
  4. operator (no id) — global view: sees everything

Exit 0 iff every check passes. No repo/app state is touched — everything lives
in a temp dir that is removed on exit.

Usage:  python3 scripts/verify-ws-isolation.py
"""
import subprocess, json, os, sys, queue, threading, shutil, pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
BIN = ROOT / "target/debug/agent-teams-mcp"
WORK = pathlib.Path("/tmp/at-ws-iso-verify")
STATE = WORK / "state"
REG = WORK / "agent-teams-live.json"
WS_A_PANE = "ws76101x0-p0"   # caller_ws = ws76101x0
WS_B_PANE = "ws88888x1-p0"   # caller_ws = ws88888x1


def build():
    if BIN.exists():
        return
    print("building sidecar (once)…")
    subprocess.run(
        ["cargo", "build", "-p", "agent-teams-mcp", "--features", "memory-notes task-tools"],
        cwd=ROOT, check=True,
    )


def send(p, obj):
    p.stdin.write(json.dumps(obj) + "\n")
    p.stdin.flush()


def session(pane_id, calls):
    """Run one server session; return {tool_name: jsonrpc_response}."""
    env = dict(os.environ)
    env["AGENT_TEAMS_STATE_DIR"] = str(STATE)
    if pane_id:
        env["AGENT_TEAMS_PANE_ID"] = pane_id
    else:
        env.pop("AGENT_TEAMS_PANE_ID", None)
    p = subprocess.Popen([str(BIN)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.DEVNULL, env=env, text=True, bufsize=1)
    assert p.stdout is not None and p.stdin is not None  # PIPE → always set
    q = queue.Queue()
    stdout = p.stdout
    threading.Thread(target=lambda: [q.put(l) for l in stdout], daemon=True).start()

    send(p, {"jsonrpc": "2.0", "id": 1, "method": "initialize",
             "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                        "clientInfo": {"name": "iso-verify", "version": "0"}}})
    send(p, {"jsonrpc": "2.0", "method": "notifications/initialized"})
    rid, want = 2, {}
    for name, args in calls:
        send(p, {"jsonrpc": "2.0", "id": rid, "method": "tools/call",
                 "params": {"name": name, "arguments": args}})
        want[rid] = name
        rid += 1

    out, got = {}, 0
    while got < len(want):
        try:
            line = q.get(timeout=8)
        except queue.Empty:
            break
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        if msg.get("id") in want:
            out[want[msg["id"]]] = msg
            got += 1
    p.terminate()
    return out


def payload(msg):
    """tools/call response → (ok, parsed_content_or_error)."""
    if "error" in msg:
        return False, msg["error"]
    res = msg.get("result", {})
    txt = res["content"][0]["text"]
    if res.get("isError"):
        return False, txt
    return True, json.loads(txt)


def main():
    build()
    shutil.rmtree(WORK, ignore_errors=True)
    STATE.mkdir(parents=True, exist_ok=True)
    checks = []

    def check(label, cond, detail=""):
        checks.append(cond)
        print(f"  [{'PASS' if cond else 'FAIL'}] {label}" + (f"  — {detail}" if detail else ""))

    print("\n[1] wsA (ws76101x0) creates a note + task")
    r = session(WS_A_PANE, [
        ("create_memory", {"title": "wsA-secret", "body": "tenant A data", "tags": [], "links": []}),
        ("task_create", {"title": "wsA-task"}),
    ])
    ok_note, note = payload(r["create_memory"])
    ok_task, task = payload(r["task_create"])
    note_id = note["note"]["id"] if ok_note else None
    task_id = task["id"] if ok_task else None
    print(f"    note={note_id}  task={task_id}")
    check("create_memory stamps ws=ws76101x0",
          ok_note and note["note"].get("workspace_id") == "ws76101x0",
          f"ws={note['note'].get('workspace_id') if ok_note else '?'}")

    print("\n[2] wsB (ws88888x1), sharing OFF — must NOT see wsA")
    r = session(WS_B_PANE, [
        ("list_memories", {}),
        ("get_memory", {"id": note_id}),
        ("task_list", {}),
        ("task_get", {"id": task_id}),
    ])
    ok, lst = payload(r["list_memories"])
    titles = [m["title"] for m in lst["memories"]] if ok else []
    check("wsB list_memories hides wsA note", "wsA-secret" not in titles, f"titles={titles}")
    ok, err = payload(r["get_memory"])
    check("wsB get_memory(wsA note) → CROSS_WORKSPACE",
          (not ok) and "CROSS_WORKSPACE" in str(err), str(err)[:90])
    ok, tl = payload(r["task_list"])
    tids = [t["id"] for t in tl["tasks"]] if ok else []
    check("wsB task_list hides wsA task", task_id not in tids, f"tasks={tids}")
    ok, err = payload(r["task_get"])
    check("wsB task_get(wsA task) → CROSS_WORKSPACE",
          (not ok) and "CROSS_WORKSPACE" in str(err), str(err)[:90])

    print("\n[3] wsA enables sharing (registry) — wsB now sees wsA")
    REG.write_text(json.dumps(
        {"schema": 1, "workspaces": [{"id": "ws76101x0", "allow_sharing": True}]}))
    r = session(WS_B_PANE, [("list_memories", {})])
    ok, lst = payload(r["list_memories"])
    titles = [m["title"] for m in lst["memories"]] if ok else []
    check("wsB sees wsA note once sharing is on", "wsA-secret" in titles, f"titles={titles}")
    REG.unlink(missing_ok=True)

    print("\n[4] operator (no AGENT_TEAMS_PANE_ID) — global view")
    r = session(None, [("list_memories", {}), ("task_list", {})])
    ok, lst = payload(r["list_memories"])
    titles = [m["title"] for m in lst["memories"]] if ok else []
    ok2, tl = payload(r["task_list"])
    tids = [t["id"] for t in tl["tasks"]] if ok2 else []
    check("operator sees wsA note", "wsA-secret" in titles, f"titles={titles}")
    check("operator sees wsA task", task_id in tids, f"tasks={tids}")

    shutil.rmtree(WORK, ignore_errors=True)
    passed = sum(checks)
    print(f"\n{'='*50}\n{passed}/{len(checks)} checks passed")
    sys.exit(0 if passed == len(checks) else 1)


if __name__ == "__main__":
    main()
