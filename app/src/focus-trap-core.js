// Agent Teams — modal focus-trap helpers (shared).
//
// Lifted out of main.js so the standalone wizard (wizard.js — imports NO Tauri/xterm)
// can trap focus in its role=dialog exactly like the app's other 9 dialogs, without
// pulling the whole app entry module in. main.js and wizard.js both import from here so
// the _focusTraps stack is a single shared source of truth (nested dialogs pop in order).
//
// Contract (mirror at every dialog): trapModalFocus(rootEl) on open, releaseModalFocus(
// rootEl) on close. Idempotent — one trap per dialog even if open is called twice.

const _focusTraps = []; // stack of { root, onKey, trigger }

function _trapFocusables(card) {
  return [...card.querySelectorAll(
    'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])'
  )].filter((el) => !el.disabled && el.getAttribute("aria-hidden") !== "true" && el.offsetParent !== null);
}

export function trapModalFocus(root) {
  if (!root || _focusTraps.some((t) => t.root === root)) return; // one trap per dialog
  const card = root.querySelector(".modal-card") || root;
  const trigger = (document.activeElement && document.activeElement !== document.body) ? document.activeElement : null;
  const onKey = (e) => {
    if (e.key !== "Tab") return;
    const f = _trapFocusables(card);
    if (!f.length) return;
    const first = f[0], last = f[f.length - 1];
    const a = document.activeElement;
    if (e.shiftKey) {
      if (a === first || !card.contains(a)) { e.preventDefault(); last.focus(); }
    } else if (a === last || !card.contains(a)) { e.preventDefault(); first.focus(); }
  };
  root.addEventListener("keydown", onKey);
  _focusTraps.push({ root, onKey, trigger });
  // Initial focus: only when the card doesn't already hold it (dialogs that rAF-focus a
  // specific field overwrite this a frame later — both paths end inside the card).
  const f = _trapFocusables(card);
  if (f.length && !card.contains(document.activeElement)) { try { f[0].focus(); } catch (_) {} }
}

export function releaseModalFocus(root) {
  for (let i = _focusTraps.length - 1; i >= 0; i--) {
    if (_focusTraps[i].root !== root) continue;
    const { onKey, trigger } = _focusTraps.splice(i, 1)[0];
    root.removeEventListener("keydown", onKey);
    if (trigger && document.contains(trigger)) { try { trigger.focus(); } catch (_) {} }
    return;
  }
}
