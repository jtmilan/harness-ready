import { describe, it, expect } from "vitest";
import { initialChat, chatReduce, classifyInput, parseSpawn, formatRoleBadge, KNOWN_HARNESSES, KNOWN_ROLES } from "./bridge-chat-core.js";

const reduceAll = (events, s = initialChat()) => events.reduce(chatReduce, s);

describe("initialChat", () => {
  it("starts idle with no turns, panes, or prd", () => {
    const s = initialChat();
    expect(s.turns).toEqual([]);
    expect(s.phase).toBe("idle");
    expect(s.paneStates).toEqual({});
    expect(s.prd).toBeNull();
  });
});

describe("chatReduce", () => {
  it("never mutates the input state", () => {
    const s = reduceAll([
      { type: "goal", text: "ship it" },
      { type: "pane", id: "a-p0", text: "working", cls: "warn" },
    ]);
    const snap = JSON.parse(JSON.stringify(s));
    chatReduce(s, { type: "goal", text: "again" });
    chatReduce(s, { type: "pane", id: "a-p0", text: "✓ report", cls: "ok" });
    chatReduce(s, { type: "phase", phase: "planning" });
    chatReduce(s, { type: "reset" });
    expect(s).toEqual(snap);
  });

  it("goal pushes a user turn", () => {
    const s = chatReduce(initialChat(), { type: "goal", text: "fix the fold" });
    expect(s.turns).toHaveLength(1);
    expect(s.turns[0]).toMatchObject({ kind: "user", text: "fix the fold" });
  });

  it("consecutive same-phase turns replace, not stack", () => {
    const s = reduceAll([
      { type: "phase", phase: "planning" },
      { type: "phase", phase: "planning", label: "still thinking" },
    ]);
    expect(s.phase).toBe("planning");
    expect(s.turns).toHaveLength(1);
    expect(s.turns[0]).toMatchObject({ kind: "phase", phase: "planning", text: "still thinking" });
  });

  it("different in-flight phases get their own turns; terminal phases set state only", () => {
    const s = reduceAll([
      { type: "phase", phase: "planning" },
      { type: "phase", phase: "collecting" },
      { type: "phase", phase: "ready" },
    ]);
    expect(s.phase).toBe("ready");
    expect(s.turns.map((t) => t.phase)).toEqual(["planning", "collecting"]);
  });

  it("pane updates fold into one live status turn", () => {
    const s = reduceAll([
      { type: "pane", id: "a-p0", text: "sent" },
      { type: "pane", id: "a-p1", text: "working", cls: "warn" },
      { type: "pane", id: "a-p0", text: "✓ report", cls: "ok" },
    ]);
    expect(s.turns).toHaveLength(1);
    expect(s.turns[0].kind).toBe("status");
    expect(s.paneStates["a-p0"]).toEqual({ text: "✓ report", cls: "ok" });
    expect(s.paneStates["a-p1"]).toEqual({ text: "working", cls: "warn" });
    expect(s.turns[0].panes).toEqual(s.paneStates);
  });

  it("a non-status turn in between starts a NEW status turn", () => {
    const s = reduceAll([
      { type: "pane", id: "a-p0", text: "sent" },
      { type: "info", text: "wave 1 done" },
      { type: "pane", id: "a-p0", text: "working" },
    ]);
    expect(s.turns.map((t) => t.kind)).toEqual(["status", "info", "status"]);
  });

  it("dispatched summarizes the send incl. dead + held verify", () => {
    const s = chatReduce(initialChat(), { type: "dispatched", n: 3, dead: 1, heldVerify: true });
    expect(s.turns[0]).toMatchObject({ kind: "dispatched", n: 3, dead: 1, heldVerify: true });
    expect(s.turns[0].text).toContain("3 panes");
    expect(s.turns[0].text).toContain("1 dead");
    expect(s.turns[0].text).toContain("verify");
  });

  it("plan and prd push card turns; prd lands in state.prd", () => {
    const s = reduceAll([
      { type: "plan", tasks: [{ id: "a-p0", task: "recon" }], twoWave: true },
      { type: "prd", path: "/tmp/final.md", verdictHint: "hold" },
    ]);
    expect(s.turns[0]).toMatchObject({ kind: "plan", twoWave: true });
    expect(s.turns[0].tasks).toHaveLength(1);
    expect(s.turns[1]).toMatchObject({ kind: "prd", path: "/tmp/final.md" });
    expect(s.prd).toMatchObject({ path: "/tmp/final.md", verdictHint: "hold" });
  });

  it("plan carries each task's typed role + owns when present (omnigent envelope)", () => {
    const s = chatReduce(initialChat(), {
      type: "plan",
      twoWave: true,
      tasks: [
        { id: "a-p0", task: "write core", wave: "code", role: "coder", owns: ["core/foo/src"] },
        { id: "a-p1", task: "review tree", wave: "verify", role: "reviewer" },
        { id: "a-p2", task: "recon", wave: "code" }, // no role → still reduces fine
      ],
    });
    const tasks = s.turns[0].tasks;
    expect(tasks[0]).toMatchObject({ id: "a-p0", role: "coder", owns: ["core/foo/src"] });
    expect(tasks[1]).toMatchObject({ id: "a-p1", role: "reviewer" });
    expect(tasks[1].owns).toBeUndefined(); // no owns supplied → field absent, not []
    expect(tasks[2]).toMatchObject({ id: "a-p2", task: "recon", wave: "code" });
    expect(tasks[2].role).toBeUndefined(); // back-compat: roleless task carries no role
  });

  it("plan back-compat: empty-string role and empty owns are dropped, not stored", () => {
    const s = chatReduce(initialChat(), {
      type: "plan",
      tasks: [{ id: "a-p0", task: "t", wave: "code", role: "", owns: [] }],
    });
    expect(s.turns[0].tasks[0].role).toBeUndefined();
    expect(s.turns[0].tasks[0].owns).toBeUndefined();
  });

  it("caps at 200 turns from the front but retains the latest plan + prd cards", () => {
    let s = reduceAll([
      { type: "plan", tasks: [], twoWave: false },
      { type: "prd", path: "/tmp/final.md" },
    ]);
    for (let i = 0; i < 250; i++) s = chatReduce(s, { type: "info", text: `tick ${i}` });
    expect(s.turns).toHaveLength(200);
    expect(s.turns.filter((t) => t.kind === "plan")).toHaveLength(1);
    expect(s.turns.filter((t) => t.kind === "prd")).toHaveLength(1);
    expect(s.turns[s.turns.length - 1].text).toBe("tick 249"); // newest never dropped
  });

  it("seq is monotonic across turns and stable on replace", () => {
    let s = reduceAll([
      { type: "goal", text: "g" },
      { type: "phase", phase: "planning" },
      { type: "phase", phase: "planning" }, // replace keeps the slot's seq
      { type: "pane", id: "a-p0", text: "sent" },
      { type: "info", text: "x" },
    ]);
    const seqs = s.turns.map((t) => t.seq);
    expect(seqs).toEqual([...seqs].sort((a, b) => a - b));
    expect(new Set(seqs).size).toBe(seqs.length);
  });

  it("reset returns a fresh state keeping nothing", () => {
    const s = reduceAll([
      { type: "goal", text: "g" },
      { type: "prd", path: "/p" },
      { type: "reset" },
    ]);
    expect(s).toEqual(initialChat());
  });

  it("unknown event type is a no-op (same state back)", () => {
    const s = chatReduce(initialChat(), { type: "goal", text: "g" });
    expect(chatReduce(s, { type: "telemetry", blob: 1 })).toBe(s);
    expect(chatReduce(s, undefined)).toBe(s);
  });
});

describe("classifyInput", () => {
  it("classifies spawn verb + known harness as spawn", () => {
    expect(classifyInput("spin up 2 codex and 1 claude")).toBe("spawn");
    expect(classifyInput("launch a cursor agent")).toBe("spawn");
  });

  it("a goal containing json braces is a goal ({status:ok} guard)", () => {
    expect(classifyInput('make the endpoint return {"status":"ok"}')).toBe("goal");
    expect(classifyInput("what's the status of the run")).toBe("goal");
  });

  it("spawn verb without a known harness stays a goal", () => {
    expect(classifyInput("spawn a background job for cleanup")).toBe("goal");
    expect(classifyInput("spin up 2 gemini")).toBe("goal");
  });

  it("harness mention without a spawn verb stays a goal", () => {
    expect(classifyInput("fix the codex selector bug")).toBe("goal");
  });
});

describe("parseSpawn", () => {
  it("parses counts (digits + words) and keeps text order", () => {
    expect(parseSpawn("spin up 2 codex and 1 claude")).toEqual({ count: 3, harnesses: ["codex", "codex", "claude"] });
    expect(parseSpawn("launch two more opencode agents")).toEqual({ count: 2, harnesses: ["opencode", "opencode"] });
  });

  it("bare harness mention defaults to count 1", () => {
    expect(parseSpawn("spawn cursor")).toEqual({ count: 1, harnesses: ["cursor"] });
  });

  it("parses cline (state-blind harness) like the other non-claude harnesses", () => {
    expect(parseSpawn("spin up 2 cline agents")).toEqual({ count: 2, harnesses: ["cline", "cline"] });
    expect(KNOWN_HARNESSES).toContain("cline");
  });

  it("returns null for garbage and unknown harnesses", () => {
    expect(parseSpawn("")).toBeNull();
    expect(parseSpawn("the quick brown fox")).toBeNull();
    expect(parseSpawn("spin up 3 gemini")).toBeNull();
  });

  it("validates against the caller-supplied harness list", () => {
    expect(parseSpawn("spin up 2 codex", ["claude"])).toBeNull();
    expect(parseSpawn("spin up 2 mybox", ["mybox"])).toEqual({ count: 2, harnesses: ["mybox", "mybox"] });
    expect(KNOWN_HARNESSES).toContain("claude");
  });
});

describe("formatRoleBadge", () => {
  it("brackets a present role and is empty for absent/blank role", () => {
    expect(formatRoleBadge("reviewer")).toBe("[reviewer]");
    expect(formatRoleBadge("  tester ")).toBe("[tester]");
    expect(formatRoleBadge("")).toBe("");
    expect(formatRoleBadge(null)).toBe("");
    expect(formatRoleBadge(undefined)).toBe("");
  });

  it("KNOWN_ROLES covers the orchestration role vocabulary", () => {
    expect(KNOWN_ROLES).toEqual(expect.arrayContaining(["coder", "tester", "coordinator", "reviewer", "scout"]));
  });
});
