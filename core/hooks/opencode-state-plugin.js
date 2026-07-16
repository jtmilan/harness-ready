// Agent Teams — opencode turn-end → state.
//
// opencode has no hook config like claude/cursor, but it DOES auto-load plugins from
// its plugins dir (~/.config/opencode/plugins/). The supervisor ensure-installs this
// file there before spawning an opencode pane. On `session.idle` (opencode's turn-end
// signal) it appends a `stop` event — the same JSONL shape state-writer.sh writes — so
// the state-adapter maps opencode → Done/TurnEnd (the synthetic SessionStart only covers
// spawn/ready).
//
// GUARDED: writes ONLY when AGENT_TEAMS_PANE_ID is set (the supervisor sets it on the
// opencode process env). For a user's own opencode session — outside agent-teams — the
// var is absent and this plugin is a complete no-op. Best-effort: never throws.

import { appendFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";

export const AgentTeamsStatePlugin = async () => {
  return {
    event: async ({ event }) => {
      try {
        if (event?.type !== "session.idle") return;
        const wsid = process.env.AGENT_TEAMS_PANE_ID;
        if (!wsid) return; // not an agent-teams pane → no-op
        const base =
          process.env.AGENT_TEAMS_STATE_DIR ||
          join(process.env.HOME || "", "Library/Application Support/harness-ready/agent-teams");
        const dir = join(base, wsid);
        mkdirSync(dir, { recursive: true });
        const line =
          JSON.stringify({
            ts: Date.now(),
            harness: "opencode",
            event: "stop",
            workspace_id: wsid,
            decision: "na",
            payload: "{}",
          }) + "\n";
        appendFileSync(join(dir, "events.jsonl"), line);
      } catch {
        // a plugin must never break the agent
      }
    },
  };
};
