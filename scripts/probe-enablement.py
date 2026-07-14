#!/usr/bin/env python3
"""Live stdio probe for the agent-write ENABLEMENT slice.

Drives the feature-enabled `agent-teams-mcp` sidecar over newline-delimited
JSON-RPC and asserts the threat-model controls that cargo-green CANNOT prove:
  VP1 (C1) — memory note-id path traversal is REJECTED; a victim file survives.
  VP2 (C2/C4) — two task_creates both survive (append-only, no lost update) and
                each genesis log line carries server-set Actor::Pane provenance.
  VP4 (C6) — an oversize task title is REJECTED (nothing appended).
  4c       — a pane CANNOT transition a task owned by another pane (scope reject);
             it CAN transition its own.
Exit 0 = all pass; non-zero on the first failure.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile

BIN = sys.argv[1]
PANE = "ws-probe-p0"
REPO_KEY = "ws-probe"

tmp = tempfile.mkdtemp(prefix="at-enable-probe-")
state = os.path.join(tmp, "state")
os.makedirs(state, exist_ok=True)
# siblings of state_root
tasks_log = os.path.join(tmp, "state-tasks-log.jsonl")
mem_dir = os.path.join(tmp, "state-memory", REPO_KEY)
os.makedirs(mem_dir, exist_ok=True)
# A victim .json OUTSIDE the per-repo memory dir, reachable only via `../` traversal.
victim = os.path.join(tmp, "state-memory", "at-probe-victim.json")
with open(victim, "w") as f:
    f.write('{"note":"do not delete me"}')

# Pre-seed a task OWNED BY ANOTHER PANE (genesis Actor::Pane{other-pane-p9}) to
# prove the 4c scope reject over the wire.
with open(tasks_log, "w") as f:
    f.write(json.dumps({
        "task_id": "seeded_other", "from": None, "to": "created",
        "by": {"kind": "pane", "workspace_id": "other-pane-p9"},
        "at": 1, "title": "owned by another pane",
    }) + "\n")

env = dict(os.environ)
env.update({
    "AGENT_TEAMS_STATE_DIR": state,
    "AGENT_TEAMS_PANE_ID": PANE,
    "AGENT_TEAMS_TASK_SCOPE": PANE,
    "AGENT_TEAMS_MEMORY_REPO_KEY": REPO_KEY,
})

proc = subprocess.Popen(
    [BIN], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    env=env, text=True, bufsize=1,
)

_id = 0
def call(method, params=None, is_notification=False):
    global _id
    msg = {"jsonrpc": "2.0", "method": method}
    if params is not None:
        msg["params"] = params
    if not is_notification:
        _id += 1
        msg["id"] = _id
    proc.stdin.write(json.dumps(msg) + "\n")
    proc.stdin.flush()
    if is_notification:
        return None
    want = _id
    while True:
        line = proc.stdout.readline()
        if not line:
            raise SystemExit(f"FAIL: sidecar closed stdout awaiting id={want}; stderr:\n{proc.stderr.read()}")
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        if obj.get("id") == want:
            return obj

def tool_call(name, arguments):
    """Returns (is_error, payload). MCP tools/call returns a result with
    content + optional isError, OR a JSON-RPC error object."""
    resp = call("tools/call", {"name": name, "arguments": arguments})
    if "error" in resp:
        return True, resp["error"]
    res = resp.get("result", {})
    return bool(res.get("isError", False)), res

fails = []
def check(cond, label):
    print(("PASS " if cond else "FAIL ") + label)
    if not cond:
        fails.append(label)

# ── handshake ──
init = call("initialize", {
    "protocolVersion": "2025-03-26",
    "capabilities": {},
    "clientInfo": {"name": "enablement-probe", "version": "0"},
})
check("result" in init and "serverInfo" in init.get("result", {}), "initialize handshake")
call("notifications/initialized", {}, is_notification=True)

# ── tools/list — the write tools must be ADVERTISED (router merge proven live) ──
tl = call("tools/list")
names = {t["name"] for t in tl.get("result", {}).get("tools", [])}
for t in ["task_list", "task_get", "task_create", "task_transition",
          "create_memory", "get_memory", "delete_memory"]:
    check(t in names, f"tools/list advertises {t}")

# ── VP1 (C1): memory id path-traversal is REJECTED; victim survives ──
err, _ = tool_call("delete_memory", {"id": "../at-probe-victim"})
check(err, "VP1 delete_memory{../} rejected")
err, _ = tool_call("get_memory", {"id": "../../etc/hosts"})
check(err, "VP1 get_memory{../../} rejected")
err, _ = tool_call("delete_memory", {"id": "/etc/hosts"})
check(err, "VP1 delete_memory{absolute} rejected")
check(os.path.exists(victim), "VP1 victim file NOT deleted by traversal")

# A WELL-FORMED memory write still works (sanity: the feature is live, not just blocked).
err, res = tool_call("create_memory", {"title": "probe", "body": "hello team"})
check(not err, "create_memory (well-formed) accepted")

# ── VP4 (C6): oversize task title rejected ──
err, _ = tool_call("task_create", {"title": "x" * 5000})
check(err, "VP4 oversize task title rejected")

# ── VP2 (C2/C4): two creates both survive + provenance ──
err, r1 = tool_call("task_create", {"title": "first task"})
check(not err, "task_create #1 accepted")
err, r2 = tool_call("task_create", {"title": "second task"})
check(not err, "task_create #2 accepted")

def struct(res):
    # rmcp returns structuredContent for object outputs; fall back to content text.
    if isinstance(res, dict) and "structuredContent" in res:
        return res["structuredContent"]
    try:
        return json.loads(res["content"][0]["text"])
    except Exception:
        return {}

id1 = struct(r1).get("id")
id2 = struct(r2).get("id")
check(bool(id1) and bool(id2) and id1 != id2, "both creates returned distinct minted ids")

# task_list unions; both created tasks visible (+ the seeded other-pane task).
err, rl = tool_call("task_list", {})
listed = {t["id"] for t in struct(rl).get("tasks", [])}
check(id1 in listed and id2 in listed, "VP2 both agent tasks visible via task_list (C4 append-only round-trip)")
check("seeded_other" in listed, "task_list unions the seeded (other-pane) task")

# provenance: read the on-disk log; the two NEW genesis lines carry Actor::Pane{PANE}.
with open(tasks_log) as f:
    log = [json.loads(l) for l in f if l.strip()]
genesis = {t["task_id"]: t for t in log if t.get("from") is None}
prov_ok = all(
    genesis.get(i, {}).get("by", {}).get("kind") == "pane"
    and genesis.get(i, {}).get("by", {}).get("workspace_id") == PANE
    for i in (id1, id2)
)
check(prov_ok, f"VP2 both genesis lines stamped Actor::Pane{{{PANE}}} (server-set C2)")
# title routed onto the genesis line (C4), not a mutable store.
check(genesis.get(id1, {}).get("title") == "first task", "VP2 title on genesis line (C4)")
check(not os.path.exists(os.path.join(tmp, "state-tasks.jsonl")),
      "VP2 mutable tasks.jsonl NOT written by agent create (C4)")

# ── 4c: scope reject (other-pane task) + scope allow (own task) ──
err, _ = tool_call("task_transition", {"id": "seeded_other", "to": "doing"})
check(err, "4c transition of ANOTHER pane's task REJECTED (scope)")
err, rt = tool_call("task_transition", {"id": id1, "to": "doing"})
check(not err, "4c transition of OWN task allowed")
check(struct(rt).get("lifecycle") == "doing", "own task advanced to doing")
# phantom id rejected
err, _ = tool_call("task_transition", {"id": "task_does_not_exist", "to": "doing"})
check(err, "phantom task id rejected (existence guard)")

proc.stdin.close()
proc.terminate()
shutil.rmtree(tmp, ignore_errors=True)

print()
if fails:
    print(f"{len(fails)} CHECK(S) FAILED:")
    for f in fails:
        print(f"  - {f}")
    sys.exit(1)
print("ALL PROBE CHECKS PASSED")
