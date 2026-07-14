// Agent Teams — layout wizard controller (Phase 1: DOM view + wiring).
//
// The CREATE flow: a 3-step modal (Start → Layout → Agents) over the #wizard markup
// (index.html) styled by styles.css. Pure state lives in wizard-core.js; presets in
// presets.js. This file is the only piece that touches the DOM — it renders off the
// core state and routes events back through the core reducers. It imports NO Tauri /
// xterm, so it loads standalone (a browser harness or jsdom can mount just the wizard).
//
// The single seam to the app is openWizard({ onCreate, defaultFolder, recents,
// defaultHarness?, count? }): main.js passes a createWorkspace wrapper as onCreate
// (it adds color + remembers last-used + closes). The add-agent flow stays on #modal
// (openAddAgent in main.js) — the wizard is purely additive.

import * as core from "./wizard-core.js";
import { listPresets, savePreset, deletePreset, getPreset, KNOWN_HARNESSES, KNOWN_ROLES } from "./presets.js";
import { trapModalFocus, releaseModalFocus } from "./focus-trap-core.js";

// Compact per-pane role labels (the wire id is on dataset.role; the full name is the
// button title). Keeps the 5-option role segment readable beside the harness segment.
const ROLE_LABELS = { none: "none", coordinator: "coord", builder: "build", scout: "scout", reviewer: "review" };

let wstate = null;      // current wizard-core state
let onCreateCb = null;  // app callback: (createArgs) => Promise
let wired = false;      // event handlers attached once

const $ = (id) => document.getElementById(id);
const STEP_TITLES = {
  1: ["Set up your workspace", "Pick a folder to work in."],
  2: ["Choose a layout", "How many terminals — pick a preset or set the count."],
  3: ["Add your agents", "Pick a harness — and an optional role — for each terminal."],
};

// "2 × claude + 2 × cursor" label for a per-pane harness list (preset auto-name).
function harnessSummary(harnesses) {
  const counts = {};
  for (const h of harnesses) counts[h] = (counts[h] || 0) + 1;
  return Object.entries(counts).map(([h, n]) => `${n} × ${h}`).join(" + ");
}

export function openWizard(opts = {}) {
  onCreateCb = opts.onCreate || null;
  wstate = core.createInitialState({
    folder: opts.defaultFolder || "",
    defaultHarness: opts.defaultHarness || core.DEFAULT_HARNESS,
    count: opts.count || 1,
    cap: opts.cap,         // Scheduler max_concurrent (D33) for the overflow hint; null = no hint
    working: opts.working, // live `working` count at open time (free slots = cap − working)
  });
  renderRecents(opts.recents || []);
  if (!wired) wireOnce();
  setError("");
  render();
  $("wizard").classList.remove("hidden");
  trapModalFocus($("wizard")); // a11y: same focus-trap contract as the app's other 9 dialogs
  // focus the folder field after the open animation settles
  requestAnimationFrame(() => { const f = $("wiz-folder"); if (f) f.focus(); });
}

export function closeWizard() { $("wizard").classList.add("hidden"); releaseModalFocus($("wizard")); }

function setError(msg) { const e = $("wiz-error"); if (e) e.textContent = msg || ""; }

// ---- rendering --------------------------------------------------------------

function render() {
  const step = wstate.step;
  const [title, sub] = STEP_TITLES[step] || STEP_TITLES[1];
  $("wiz-title").textContent = title;
  $("wiz-sub").textContent = sub;

  // step indicator: current = .active, earlier = .done
  document.querySelectorAll("#wiz-steps .wiz-step-dot").forEach((dot) => {
    const n = parseInt(dot.dataset.step, 10);
    dot.classList.toggle("active", n === step);
    dot.classList.toggle("done", n < step);
  });
  // sections: only the active one shows
  [1, 2, 3].forEach((n) => $("wiz-s" + n).classList.toggle("hidden", n !== step));

  // inputs are user-driven — only write when not focused (avoid clobbering a caret)
  syncInput("wiz-name", wstate.name);
  syncInput("wiz-folder", wstate.folder);
  syncInput("wiz-prompt", wstate.seedPrompt);

  if (step === 2) renderLayout();
  if (step === 3) renderAgents();

  // footer
  $("wiz-back").style.display = step === 1 ? "none" : "";
  $("wiz-noai").style.display = step === 3 ? "none" : ""; // mockup: "Open without AI" before the agents step
  $("wiz-next").textContent = core.isLastStep(wstate) ? "Create workspace" : "Next";
}

function syncInput(id, value) {
  const el = $(id);
  if (el && document.activeElement !== el && el.value !== value) el.value = value;
}

function renderRecents(recents) {
  const box = $("wiz-recents");
  if (!box) return;
  box.replaceChildren();
  for (const f of recents) {
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = "recent-chip";
    chip.textContent = f;
    chip.title = f;
    chip.onclick = () => { wstate = core.setFolder(wstate, f); $("wiz-folder").value = f; setError(""); };
    box.appendChild(chip);
  }
}

function renderLayout() {
  // count tiles
  document.querySelectorAll("#wiz-count .count-tile").forEach((b) =>
    b.classList.toggle("active", parseInt(b.dataset.n, 10) === wstate.count));
  $("wiz-count-label").textContent = wstate.count + (wstate.count === 1 ? " terminal" : " terminals");

  // grid-shape preview: one dot per pane, columns driven by --wiz-cols
  const prev = $("wiz-grid-preview");
  const { cols } = core.gridShape(wstate.count);
  prev.style.setProperty("--wiz-cols", String(cols));
  prev.replaceChildren();
  for (let i = 0; i < wstate.count; i++) {
    const d = document.createElement("div");
    d.className = "wiz-grid-dot";
    prev.appendChild(d);
  }

  renderCapHint();
  renderPresets();
}

// Cap-aware overflow hint (D33): when the chosen pane count exceeds the free working
// slots, the backend QUEUES the overflow (not an error — {queued,position} is success);
// tell the user so a parked pane isn't a surprise. Hidden when nothing overflows or the
// cap is unknown. Recomputed on every renderLayout() → updates as the count tile changes.
function renderCapHint() {
  const el = $("wiz-cap-hint");
  if (!el) return;
  const over = core.overflowCount(wstate);
  if (over <= 0) { el.textContent = ""; el.classList.add("hidden"); return; }
  const free = Math.max(0, wstate.cap - wstate.working);
  el.textContent = `${wstate.count} terminals — ${over} will queue (only ${free} of ${wstate.cap} concurrency slots free). Queued agents start automatically as slots free up.`;
  el.classList.remove("hidden");
}

// Render preset chips BEFORE the static #wiz-preset-new button (never innerHTML-reset
// #wiz-presets — that would destroy + NEW). Remove only previously-rendered chips.
function renderPresets() {
  const row = $("wiz-presets");
  const newBtn = $("wiz-preset-new");
  row.querySelectorAll(".wiz-preset").forEach((c) => c.remove());
  for (const p of listPresets()) {
    const chip = document.createElement("div");
    // unspawnable = references a harness this build can't spawn (presets.js, forward-
    // compat for a removed harness). Disable apply (not delete — a broken USER preset
    // must stay removable), don't crash. Apply is gated in applyPresetById.
    chip.className = "wiz-preset"
      + (p.id === wstate.activePresetId ? " active" : "")
      + (p.unspawnable ? " disabled" : "");
    chip.dataset.id = p.id;
    chip.setAttribute("role", "button"); // keyboard-applicable (Enter/Space) — see wireOnce
    chip.tabIndex = p.unspawnable ? -1 : 0; // unreachable by Tab when not applicable
    if (p.unspawnable) chip.setAttribute("aria-disabled", "true");
    const unknown = p.unspawnable ? p.harnesses.filter((h) => !KNOWN_HARNESSES.includes(h)) : [];
    chip.title = p.unspawnable
      ? `Unavailable — unknown harness: ${unknown.join(", ")}`
      : `${p.name} — ${harnessSummary(p.harnesses)}`;
    const label = document.createElement("span");
    label.textContent = p.name;
    chip.appendChild(label);
    if (!p.builtin) {
      const del = document.createElement("button");
      del.type = "button";
      del.className = "wiz-preset-del";
      del.textContent = "🗑";
      del.title = "Delete preset";
      del.dataset.del = p.id;
      chip.appendChild(del);
    }
    row.insertBefore(chip, newBtn);
  }
}

function renderAgents() {
  const grid = $("wiz-harness-grid");
  grid.replaceChildren();
  const roles = wstate.roles || [];
  wstate.harnesses.forEach((h, i) => {
    const rowEl = document.createElement("div");
    rowEl.className = "wiz-pane-row";
    const span = document.createElement("span");
    span.textContent = "Terminal " + (i + 1);

    // harness + role segments stack vertically in a controls column (the row stays
    // [label | controls]).
    const controls = document.createElement("div");
    controls.className = "wiz-pane-controls";

    const seg = document.createElement("div");
    seg.className = "wiz-harness-seg";
    seg.dataset.i = String(i);
    seg.setAttribute("role", "group");
    seg.setAttribute("aria-label", "Harness for terminal " + (i + 1));
    for (const hid of KNOWN_HARNESSES) {
      const opt = document.createElement("button");
      opt.type = "button";
      opt.className = "wiz-harness-opt" + (hid === h ? " active" : "");
      opt.dataset.h = hid;
      opt.textContent = hid;
      opt.setAttribute("aria-pressed", hid === h ? "true" : "false");
      seg.appendChild(opt);
    }

    // 17-01: per-pane ROLE — a segmented control (none | coord | build | scout |
    // review), NOT a dropdown. Mirrors the harness segment; "none" = roleless.
    const r = roles[i] || core.DEFAULT_ROLE;
    const rseg = document.createElement("div");
    rseg.className = "wiz-role-seg";
    rseg.dataset.i = String(i);
    rseg.setAttribute("role", "group");
    rseg.setAttribute("aria-label", "Role for terminal " + (i + 1));
    for (const rid of KNOWN_ROLES) {
      const opt = document.createElement("button");
      opt.type = "button";
      opt.className = "wiz-role-opt" + (rid === r ? " active" : "");
      opt.dataset.role = rid;
      opt.textContent = ROLE_LABELS[rid] || rid;
      opt.title = rid === "none" ? "no role (homogeneous)" : rid;
      opt.setAttribute("aria-pressed", rid === r ? "true" : "false");
      rseg.appendChild(opt);
    }

    // model-at-spawn: per-pane MODEL — a free-text input (NOT a segment: real lists run
    // 100+ ids), backed by the global per-harness <datalist> (dl-models-<h>, filled by
    // main.js's ensureModelsLoaded). Blank = account default. Re-rendered on harness
    // change so the autocomplete follows the pane's harness.
    const mInp = document.createElement("input");
    mInp.type = "text";
    mInp.className = "wiz-model-input";
    mInp.dataset.i = String(i);
    mInp.setAttribute("list", "dl-models-" + h);
    mInp.placeholder = "model (optional — account default)";
    mInp.autocomplete = "off";
    mInp.spellcheck = false;
    mInp.value = (wstate.models && wstate.models[i]) || "";
    mInp.setAttribute("aria-label", "Model for terminal " + (i + 1));

    controls.appendChild(seg);
    controls.appendChild(rseg);
    controls.appendChild(mInp);
    rowEl.appendChild(span);
    rowEl.appendChild(controls);
    grid.appendChild(rowEl);
  });
}

// ---- events (wired once) ----------------------------------------------------

function wireOnce() {
  wired = true;

  $("wiz-cancel").onclick = closeWizard;
  $("wiz-back").onclick = () => { wstate = core.goBack(wstate); setError(""); render(); };
  $("wiz-next").onclick = onNext;
  $("wiz-noai").onclick = () => doCreate(core.openWithoutAI(wstate));

  // raw field inputs update state without a re-render (no layout impact)
  $("wiz-name").oninput = (e) => { wstate = core.setName(wstate, e.target.value); };
  $("wiz-folder").oninput = (e) => { wstate = core.setFolder(wstate, e.target.value); if (e.target.value.trim()) setError(""); };
  $("wiz-prompt").oninput = (e) => { wstate = core.setSeedPrompt(wstate, e.target.value); };

  // count tiles
  document.querySelectorAll("#wiz-count .count-tile").forEach((btn) => {
    btn.onclick = () => { wstate = core.setCount(wstate, parseInt(btn.dataset.n, 10) || 1); render(); };
  });

  // presets row (delegated): delete vs apply
  $("wiz-presets").onclick = (e) => {
    const delId = e.target.closest(".wiz-preset-del")?.dataset.del;
    if (delId) {
      try { deletePreset(delId); } catch (_) { /* builtin guard — never offered, ignore */ }
      if (wstate.activePresetId === delId) wstate = { ...wstate, activePresetId: null };
      renderPresets();
      return;
    }
    const chip = e.target.closest(".wiz-preset");
    if (chip && chip.dataset.id) applyPresetById(chip.dataset.id);
  };
  // keyboard: preset chips are role=button → Enter/Space applies (🗑 is its own button)
  $("wiz-presets").addEventListener("keydown", (e) => {
    if (e.key !== "Enter" && e.key !== " ") return;
    if (e.target.closest(".wiz-preset-del")) return;
    const chip = e.target.closest(".wiz-preset");
    if (chip && chip.dataset.id) { e.preventDefault(); applyPresetById(chip.dataset.id); }
  });

  // + NEW: jump to the agents step to build a fresh layout, then "Save as preset"
  $("wiz-preset-new").onclick = () => { wstate = core.setStep({ ...wstate, activePresetId: null }, 3); render(); };

  // per-pane harness + role — clickable segmented buttons (delegated). Both live in
  // #wiz-harness-grid; flip active within just the clicked segment (no grid rebuild →
  // no flicker / focus loss).
  $("wiz-harness-grid").onclick = (e) => {
    const hOpt = e.target.closest(".wiz-harness-opt");
    if (hOpt) {
      const seg = hOpt.closest(".wiz-harness-seg");
      const pi = parseInt(seg.dataset.i, 10);
      wstate = core.setPaneHarness(wstate, pi, hOpt.dataset.h);
      seg.querySelectorAll(".wiz-harness-opt").forEach((b) => {
        const on = b.dataset.h === hOpt.dataset.h;
        b.classList.toggle("active", on);
        b.setAttribute("aria-pressed", on ? "true" : "false");
      });
      // model-at-spawn: retarget the pane's model autocomplete at the new harness;
      // a typed id is harness-specific so a switch clears it.
      const mInp = seg.closest(".wiz-pane-controls")?.querySelector(".wiz-model-input");
      if (mInp) {
        mInp.setAttribute("list", "dl-models-" + hOpt.dataset.h);
        mInp.value = "";
        wstate = core.setPaneModel(wstate, pi, "");
      }
      renderPresets(); // a manual harness change desyncs the active preset → clear highlight
      return;
    }
    const rOpt = e.target.closest(".wiz-role-opt");
    if (rOpt) {
      const seg = rOpt.closest(".wiz-role-seg");
      wstate = core.setPaneRole(wstate, parseInt(seg.dataset.i, 10), rOpt.dataset.role);
      seg.querySelectorAll(".wiz-role-opt").forEach((b) => {
        const on = b.dataset.role === rOpt.dataset.role;
        b.classList.toggle("active", on);
        b.setAttribute("aria-pressed", on ? "true" : "false");
      });
      // a role pick does NOT desync the active preset (presets are harness-only today).
    }
  };

  // model-at-spawn: per-pane model typing → state (delegated input event; no re-render)
  $("wiz-harness-grid").addEventListener("input", (e) => {
    const mInp = e.target.closest(".wiz-model-input");
    if (mInp) wstate = core.setPaneModel(wstate, parseInt(mInp.dataset.i, 10), mInp.value.trim());
  });

  // Save current layout as a user preset
  $("wiz-save-preset").onclick = onSavePreset;

  // keyboard: Enter = Next/Create (except inside the textarea), Esc = Cancel
  $("wizard").addEventListener("keydown", (e) => {
    if (e.key === "Escape") { e.preventDefault(); closeWizard(); return; }
    // Enter advances only from a text input — NOT from buttons/selects (whose own
    // Enter-to-activate we must not hijack) and not the textarea (keeps newlines).
    if (e.key === "Enter" && e.target.tagName === "INPUT") { e.preventDefault(); onNext(); }
  });
}

function onNext() {
  if (core.isLastStep(wstate)) { doCreate(wstate); return; }
  const { ok, reason } = core.canAdvance(wstate);
  if (!ok) { setError(reason); return; }
  setError("");
  wstate = core.goNext(wstate);
  render();
}

function onSavePreset() {
  try {
    const name = wstate.name.trim() || harnessSummary(wstate.harnesses);
    const p = savePreset({
      name,
      harnesses: wstate.harnesses.slice(),
      folder: wstate.folder.trim() || undefined,
      seedPrompt: wstate.seedPrompt || undefined,
    });
    wstate = { ...wstate, activePresetId: p.id };
    setError("");
    renderPresets();
    flashSaved(); // the new chip lives on step 2 (hidden here) — confirm inline
  } catch (err) {
    setError(String(err && err.message ? err.message : err));
  }
}

// Briefly confirm a save on the button itself (the presets row it joins is on the
// hidden step 2, so the chip appearing isn't visible feedback on step 3).
let savedFlashTimer = null;
function flashSaved() {
  const b = $("wiz-save-preset");
  if (!b) return;
  b.textContent = "✓ Saved to presets";
  if (savedFlashTimer) clearTimeout(savedFlashTimer);
  savedFlashTimer = setTimeout(() => { b.textContent = "＋ Save as preset"; }, 1600);
}

// Apply a preset by id, then land on the Agents step as a review (NOT auto-spawn).
function applyPresetById(id) {
  const p = getPreset(id);
  // An unspawnable preset (references a removed/unknown harness) is inert — both the
  // click and keydown paths funnel here, so this one guard covers both.
  if (!p || p.unspawnable) return;
  wstate = core.setStep(core.applyPreset(wstate, p), 3); setError(""); render();
}

async function doCreate(state) {
  const args = core.toCreateArgs(state);
  if (!args.repo) { setError("working folder is required"); wstate = core.setStep(state, 1); render(); return; }
  if (!onCreateCb) { setError("create handler unavailable"); return; }
  // await BEFORE closing: a create error then keeps the wizard open + visible (closing
  // would hide it — the legacy spawn path writes errors to the now-hidden #f-error).
  try { await onCreateCb(args); closeWizard(); }
  catch (err) { setError(String(err && err.message ? err.message : err)); }
}
