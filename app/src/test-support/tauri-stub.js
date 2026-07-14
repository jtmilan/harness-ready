// Agent Teams — test-support: install a fake `window.__TAURI__` for DOM tests.
//
// The app reads `window.__TAURI__.core.invoke` DIRECTLY (main.js `tauriInvoke`), NOT
// `@tauri-apps/api`. So DOM tests can't mock a module — they must plant the global.
// This helper installs a configurable stub driven by a per-test fixtures map and returns
// a handle so a test can assert calls, drive listen() callbacks, and tear the global
// back down.
//
// Dependency-free: imports only `vi` from vitest (no app code, no `@tauri-apps/*`).
import { vi } from "vitest";

// installTauriStub(fixtures) → handle
//
// fixtures = {
//   invoke: { <command>: value | (args) => value | Promise<value> },  // unknown cmd rejects
//   listen: { <event>:  (payload) => void } (unused hook; real driving is via handle.emit)
// }
//
// handle = {
//   invoke,   // the vi.fn() planted at window.__TAURI__.core.invoke (assert .mock.calls)
//   listen,   // the vi.fn() planted at window.__TAURI__.event.listen
//   emit(event, payload),  // fire every listen()-registered callback for `event`
//   restore(),             // put window.__TAURI__ back to what it was (or delete it)
// }
export function installTauriStub(fixtures = {}) {
  const commands = fixtures.invoke || {};
  const listeners = new Map(); // event -> Set<callback>

  // Mirrors the real backend contract: a known command resolves (value or handler
  // result), an unknown command rejects — so tests can assert the app only calls
  // commands it declared a fixture for.
  const invoke = vi.fn(async (cmd, args) => {
    if (!Object.prototype.hasOwnProperty.call(commands, cmd)) {
      throw new Error(`tauri-stub: no fixture for invoke("${cmd}")`);
    }
    const handler = commands[cmd];
    return typeof handler === "function" ? handler(args) : handler;
  });

  // window.__TAURI__.event.listen(event, cb) → Promise<unlisten>. The stub records the
  // callback so handle.emit() can drive it, and the returned unlisten removes it.
  const listen = vi.fn(async (event, cb) => {
    if (!listeners.has(event)) listeners.set(event, new Set());
    listeners.get(event).add(cb);
    return () => {
      const set = listeners.get(event);
      if (set) set.delete(cb);
    };
  });

  const prev = Object.prototype.hasOwnProperty.call(window, "__TAURI__")
    ? window.__TAURI__
    : undefined;
  const had = Object.prototype.hasOwnProperty.call(window, "__TAURI__");
  window.__TAURI__ = { core: { invoke }, event: { listen } };

  return {
    invoke,
    listen,
    emit(event, payload) {
      const set = listeners.get(event);
      if (set) for (const cb of Array.from(set)) cb({ event, payload });
    },
    restore() {
      if (had) window.__TAURI__ = prev;
      else delete window.__TAURI__;
    },
  };
}
