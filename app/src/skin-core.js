// skin-core — the pure themeable-skin logic extracted from main.js (pattern:
// svg-icon-core.js / diff-view-core.js — pure module, imported back, unit-tested).
// main.js keeps only the DOM/terminal side effects (attribute swap, localStorage I/O,
// xterm re-theme); every DECISION lives here so vitest can pin it down in a node env.
//
// ⚠ SYNC NOTE: index.html carries a PRE-PAINT inline copy of this allowlist (the
// <script> in <head>, ~line 18: `var allow = ["aurora", …]`) so a persisted /
// deep-linked skin applies before first paint. That script runs pre-module, so it
// CANNOT import this file — when SKINS changes, update the index.html copy by hand
// (it intentionally omits "nothing", which is expressed as NO data-skin attribute).
export const SKINS = ["nothing", "aurora", "atelier", "phosphor", "precision", "liquid-glass"];
export const SKIN_KEY = "at_skin";

export function isValidSkin(name) { return SKINS.includes(name); }

// Any input → a valid skin name. Unknown/absent collapses to the default "nothing".
export function normalizeSkin(name) { return isValidSkin(name) ? name : "nothing"; }

// Active skin resolution order: (a) ?skin= URL param, (b) persisted value, (c) default
// "nothing". Pure: takes the location.search string + the already-read stored value
// (main.js does the localStorage read, with its try/catch, at the call site).
export function resolveBootSkin(search, stored) {
  try {
    const q = new URLSearchParams(search || "").get("skin");
    if (q && isValidSkin(q)) return q;
  } catch (_) {}
  if (stored && isValidSkin(stored)) return stored;
  return "nothing";
}

// The delete-vs-set decision for html[data-skin]: "nothing" is the BARE :root (no
// attribute at all), every other skin sets the attribute. Returns the attribute value
// to set, or null → REMOVE the attribute.
export function skinAttrFor(name) {
  const skin = normalizeSkin(name);
  return skin === "nothing" ? null : skin;
}

// The persistence decision for a live skin switch: the default skin REMOVES the key
// (so a fresh profile and a reset-to-default profile are indistinguishable), any other
// skin sets it. Returns { action: "remove" } | { action: "set", value }.
export function skinStorageOp(name) {
  const skin = normalizeSkin(name);
  return skin === "nothing" ? { action: "remove" } : { action: "set", value: skin };
}
