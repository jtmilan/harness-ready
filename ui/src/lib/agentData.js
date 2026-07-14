import { randomAgentName } from "@/lib/agentNames";

export const STATUS_META = {
  needs_input: { badge: "!", color: "text-amber-300", glow: "shadow-[0_0_10px_rgba(251,191,36,0.5)]", label: "NEEDS YOU" },
  working: { badge: "W", color: "text-cyan-300", glow: "shadow-[0_0_8px_rgba(0,229,255,0.4)]", label: "WORKING" },
  starting: { badge: "S", color: "text-yellow-300", glow: "", label: "STARTING" },
  blocked: { badge: "B", color: "text-orange-400", glow: "", label: "BLOCKED" },
  error: { badge: "E", color: "text-red-400", glow: "", label: "ERROR" },
  idle: { badge: "I", color: "text-slate-500", glow: "", label: "IDLE" },
};

export const ATTENTION_REASONS = [
  "Permission needed: run `cargo test --workspace`?",
  "Plan approval required before applying 6-file diff",
  "Merge conflict in worktree — manual resolution needed",
  "Choose implementation strategy: (A) refactor (B) patch",
  "Tool call confirmation: `rm -rf target/`",
  "Agent finished task — review & assign next step",
];

const OUTPUT_LINES = [
  "$ agent status --live",
  "worktree: ~/worktrees/ws28151x0",
  "compiling module core::exec ... ok",
  "task queued: refactor session handler",
  "resolving deps [cargo] 148/212",
  "PATCH applied src/pane/render.rs",
  "test suite: 96 passed, 0 failed",
  "streaming tokens ... 1.2k/s",
  "checkpoint saved @ HEAD~0",
  "diff staged: +214 -89 lines",
  "spawning subprocess pid=4821",
  "lint clean — 0 warnings",
  "merging worktree changes ... done",
  "awaiting orchestrator signal",
  "context window: 62% utilized",
  "fetching remote refs ... ok",
  "$ cargo build --release",
  "indexing codebase 3,412 files",
];

export function randomLine() {
  return OUTPUT_LINES[Math.floor(Math.random() * OUTPUT_LINES.length)];
}

const INITIAL = [
  { kind: "claude-code", status: "working" },
  { kind: "cursor", status: "needs_input" },
  { kind: "opencode", status: "working" },
  { kind: "codex", status: "idle" },
  { kind: "grok", status: "working" },
  { kind: "claude-code", status: "needs_input" },
  { kind: "commandcode", status: "starting" },
  { kind: "cline", status: "blocked" },
  { kind: "claude-code", status: "working" },
  { kind: "bash", status: "idle" },
  { kind: "cursor", status: "working" },
  { kind: "codex", status: "error" },
];

const ATTENTION_STATUSES = ["needs_input", "blocked", "error"];

export function createAgents(count = 12) {
  return INITIAL.slice(0, count).map((cfg, i) => {
    const num = String(i + 1).padStart(3, "0");
    return {
      id: `AGENT-${num}`,
      name: randomAgentName(),
      kind: cfg.kind,
      branch: `feat/${cfg.kind}-${i + 1}`,
      worktree: `~/worktrees/agent-${num}`,
      status: cfg.status,
      attention: ATTENTION_STATUSES.includes(cfg.status)
        ? { reason: ATTENTION_REASONS[i % ATTENTION_REASONS.length], since: Date.now() - Math.floor(Math.random() * 240000) }
        : null,
      output: Array.from({ length: 3 + Math.floor(Math.random() * 5) }, randomLine),
    };
  });
}