// Registry of supported interactive coding agents.
// `cmd` is the CLI the local backend spawns inside the agent's PTY.
export const AGENT_KINDS = {
  "claude-code": { label: "CLAUDE CODE", cmd: "claude" },
  cursor: { label: "CURSOR", cmd: "cursor-agent" },
  opencode: { label: "OPENCODE", cmd: "opencode" },
  codex: { label: "CODEX", cmd: "codex" },
  commandcode: { label: "COMMANDCODE", cmd: "commandcode" },
  pi: { label: "PI", cmd: "pi" },
  grok: { label: "GROK", cmd: "grok" },
  bash: { label: "BASH", cmd: "bash" },
};

export const KIND_IDS = Object.keys(AGENT_KINDS);
