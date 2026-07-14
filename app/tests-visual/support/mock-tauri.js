// Self-contained addInitScript payload: plants a fake `window.__TAURI__` BEFORE the app's
// modules run, so the frontend boots to its empty shell with no real backend. Must be a
// standalone function (Playwright serializes it — no closures over imports).
export function installMockTauri() {
  const EMPTY = {
    list_queue: [],
    list_workspaces: [],
    dead_pane_ids: [],
    list_mcp_tasks: [],
    list_loops: [],
    list_delegations: [],
    run_history: [],
    delegate_gate_status: {
      allow_mutations: false,
      send_input_enabled: false,
      http_enabled: false,
      daemon_spawn_enabled: false,
      loop_autonomy: false,
    },
    mcp_http_status: { enabled: false },
    session_cost: { usd: 0, tokens: 0 },
  };
  window.__TAURI__ = {
    core: {
      invoke: (cmd) =>
        Promise.resolve(Object.prototype.hasOwnProperty.call(EMPTY, cmd) ? EMPTY[cmd] : null),
    },
    event: { listen: () => Promise.resolve(() => {}) },
  };
}
