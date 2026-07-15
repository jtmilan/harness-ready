// ---- Per-pane DISPLAY label (rename) ------------------------------------------------------
// Display-only. NEVER the machine pane id — that stays the git-safe `ws…-pN` the worktrees and
// branches are cut from. Mirrors the shape of prod's `at_pane_labels` store
// (agent-teams/app/src/main.js:221) under this fork's `hr:` key namespace. NEW code here, not a
// port: prod reads a module global and repaints by hand; this exposes a subscription so React
// panes re-render themselves.
//
// This module owns its storage end to end — read, write, and notify. That is deliberate and is
// what keeps AgentPane presentational (BRIEF C2): the pane calls `usePaneLabel`/`setPaneLabel`
// and never sees localStorage, and this file stays the single writer of the key.
//
// Best-effort throughout: a storage failure degrades to "no custom label" (and, for a failed
// write, to a label that lives only for this session). It can never wedge a pane, because the
// label is fully isolated from the PTY/session lifecycle.
import { useSyncExternalStore } from "react";

const KEY = "hr:pane-labels";

/** @type {Record<string, string>} paneId -> custom label. Replaced (never mutated) on write. */
let labels = load();
const listeners = new Set();

function load() {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) || "{}");
    if (saved && typeof saved === "object" && !Array.isArray(saved)) return saved;
  } catch {
    /* unreadable / corrupt / private mode — fall through to an empty map */
  }
  return {};
}

function persist() {
  try {
    localStorage.setItem(KEY, JSON.stringify(labels));
  } catch {
    /* quota or private mode: the in-memory map still holds for this session */
  }
}

function subscribe(fn) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/**
 * A pane's custom label, or null when it has none.
 *
 * Returns the OVERRIDE ONLY — the caller owns the fallback. Prod falls back to the bare id
 * (`main.js:224`), but a pane here has a richer default (`agent.name || agent.id`) that this
 * module has no business knowing about.
 *
 * @param {string} id pane id
 * @returns {string|null}
 */
export function getPaneLabel(id) {
  const v = labels[id];
  return typeof v === "string" && v ? v : null;
}

/**
 * Set — or clear — a pane's custom label, then notify subscribers.
 *
 * Empty/whitespace, or a label identical to the pane id, CLEARS the override instead of storing
 * a redundant one (prod parity, `main.js:225`). Renaming to the current value is a no-op: it
 * neither churns storage nor wakes subscribers.
 *
 * @param {string} id pane id
 * @param {string} label new label ("" / whitespace clears)
 */
export function setPaneLabel(id, label) {
  const v = (label || "").trim();
  const next = v && v !== id ? v : null;
  if (getPaneLabel(id) === next) return;
  const map = { ...labels };
  if (next) map[id] = next;
  else delete map[id];
  labels = map;
  persist();
  for (const fn of [...listeners]) fn();
}

/**
 * Subscribe a component to ONE pane's label. Re-renders on any change to that pane's label,
 * whoever committed it.
 *
 * @param {string} id pane id
 * @returns {string|null} the override, or null when the pane has no custom label
 */
export function usePaneLabel(id) {
  // getSnapshot returns a string|null primitive, so it is referentially stable between
  // unrelated renders and cannot loop useSyncExternalStore.
  return useSyncExternalStore(subscribe, () => getPaneLabel(id));
}
