// settings-core — pure/testable helpers extracted from the Settings modal.
// Keeps the two-click confirm decision and the trusted-repo list DOM builder out of the
// 7000-line main.js so they can be unit-tested (node env for the decision, happy-dom for
// the renderer).

// Two-click add-confirm decision for the trusted-repos manager.
//   pending   — repo path awaiting the confirming 2nd Add click (null when unarmed)
//   pendingAt — timestamp of the 1st Add click
//   now       — current time (ms)
//   path      — the path being added on this click
// Returns { action, pending, pendingAt } — the caller applies `pending`/`pendingAt` to its state.
//   "confirm" → the 2nd click on the same path inside the 6s window; state is disarmed.
//   "arm"     → first click (or window expired, or a different path); state re-armed for `path`.
export const TRUSTED_ADD_CONFIRM_MS = 6000;

export function trustedAddDecision(pending, pendingAt, now, path) {
  if (pending === path && now - pendingAt < TRUSTED_ADD_CONFIRM_MS) {
    return { action: "confirm", pending: null, pendingAt: 0 };
  }
  return { action: "arm", pending: path, pendingAt: now };
}

// (Re)build the trusted-repo list into `container`. Each row = path label + Remove button.
// `onRemove(path)` is wired to each Remove button's click. Empty list → an empty-state note.
// Pure-ish: no globals — uses container.ownerDocument so it works under happy-dom + the browser.
export function renderTrustedReposList(container, repos, onRemove) {
  const doc = container.ownerDocument;
  container.replaceChildren();
  const list = Array.isArray(repos) ? repos : [];
  if (!list.length) {
    const empty = doc.createElement("p");
    empty.className = "modal-sub";
    empty.style.cssText = "margin:0;opacity:0.75;";
    empty.textContent = "No trusted repositories yet.";
    container.appendChild(empty);
    return container;
  }
  for (const path of list) {
    const row = doc.createElement("div");
    row.style.cssText =
      "display:flex;align-items:center;gap:var(--s-2);padding:var(--s-1) var(--s-2);" +
      "border:1px solid var(--line);border-radius:var(--r-2);background:var(--bg);";
    const p = doc.createElement("span");
    p.textContent = path;
    p.title = path;
    p.style.cssText =
      "flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;" +
      "direction:rtl;text-align:left;font-family:var(--font-mono);font-size:12px;color:var(--fg);";
    const rm = doc.createElement("button");
    rm.type = "button";
    rm.textContent = "Remove";
    rm.setAttribute("aria-label", "Remove trusted repository " + path);
    rm.style.cssText =
      "padding:2px var(--s-2);border-radius:var(--r-1);border:1px solid var(--line-strong);" +
      "background:transparent;color:var(--fg-secondary);cursor:pointer;font-size:11px;white-space:nowrap;";
    if (typeof onRemove === "function") rm.addEventListener("click", () => onRemove(path));
    row.append(p, rm);
    container.appendChild(row);
  }
  return container;
}
