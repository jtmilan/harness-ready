// useKeyboardShortcuts — the app's global keyboard shortcuts. This is the ONLY global keydown
// listener in the app: `components/ui/sidebar.jsx` has one that looks like a peer, but
// `SidebarProvider` is never mounted, so that code is dead. Do not extend it.
//
//   ⌘⇧I → onBroadcastToggle()   ⌘G → onMaximizeToggle()
//
// Platform: macOS-only by ship target, not by accident. Bundle targets are `app`+`dmg`
// (app/src-tauri/tauri.conf.json), sidecars are aarch64-apple-darwin only, install scripts
// write into Contents/MacOS, and the sibling prod dispatcher is entirely metaKey-based
// (app/src/main.js:8290-8364). No Ctrl+Shift fallback — Ctrl+Shift+I is the Windows/Linux
// webview devtools chord, and there is no non-mac ship surface today. If bundle targets
// ever grow past macOS, revisit this gate with a non-devtools chord, not a blind ctrlKey OR.
//
// Bound on `document` in the BUBBLE phase, which is load-bearing rather than incidental:
// these shortcuts must keep working while focus is inside an xterm, and xterm listens on its
// own textarea. Bubble phase lets the terminal see the key first and only reach us if it
// declines it — which for ⌘-combos it always does (see the xterm note below).
//
// Deliberately NOT guarded by a "is the user typing" check. Prod's dispatcher
// (agent-teams/app/src/main.js:8290-8364) guards only ⌥↑/↓ and F2 with `_isTypingTarget`,
// and leaves every ⌘ combo unguarded; we match that. A typing guard here would break the
// primary use case, since a focused terminal is exactly when you reach for broadcast.
//
// ── Matching semantics ────────────────────────────────────────────────────────────────
// BOTH arms match on physical `e.code` ("KeyI" / "KeyG"), never on `e.key`. Both reject
// e.repeat and call preventDefault + stopPropagation on hit. Modifier gates DIFFER:
//   - ⌘⇧I (broadcast): meta + shift, no alt/ctrl
//   - ⌘G  (maximize):  meta + NO shift, no alt/ctrl
//
// Why ⌘G not ⌘⇧G (operator-confirmed, wave-8):
//   ⌘⇧G is swallowed by WKWebView (Find-Previous family) before the page OR the app menu
//   sees it — menu *click* worked; the chord never did. Plain ⌘G is delivered to page JS
//   in this webview class (prod main.js:8314 ⌘G toggleGrid works with terminals focused).
//   Native menu accelerator is the same CmdOrCtrl+G (lib.rs Pane ▸ Toggle Pane Zoom).
//
// Double-path contract (in-app Tauri):
//   WKWebView forwards the keydown to the page first. Our preventDefault stops menu
//   re-dispatch of the accelerator → exactly one toggle per press. In the browser preview
//   there is no app menu → JS arm alone. Menu click still maximizes when focus is elsewhere.
//
// Why `e.code` (not `e.key`):
//   - Shift alone uppercases letter keys on macOS QWERTY (`e.key` → "G"/"I"), which a
//     lower-case `e.key === "g"` check would miss — but does NOT change `e.code`.
//   - Option remaps `e.key` entirely (⌥G → "©"); prod hit this and switched to
//     `e.code === "KeyG"` (main.js:8312). `e.code` is also layout-independent.
//
// Chrome-preview note: browser Find-Next is ⌘G. We preventDefault on match so Chrome's
// find bar should not open while the app is focused — INFERRED if not re-checked live.
// In-app WKWebView the native find family was the old ⌘⇧G problem; ⌘G is page-delivered.

import * as React from "react";

/**
 * @typedef {object} ShortcutHandlers
 * @property {() => void} [onBroadcastToggle] ⌘⇧I — toggle broadcast-to-all-panes mode.
 * @property {() => void} [onMaximizeToggle]  ⌘G — toggle zoom on the highlighted pane.
 */

/**
 * Bind the app's global shortcuts for the lifetime of the calling component.
 * @param {ShortcutHandlers} [handlers]
 */
export function useKeyboardShortcuts(handlers = {}) {
  // Latest-ref: consumers pass inline arrows, so `handlers` is a new object every render.
  // Reading through a ref keeps the listener identity stable (bound once on mount) while
  // still dispatching to the current closures — rebinding document listeners on every
  // render would be pure churn.
  const latest = React.useRef(handlers);
  React.useEffect(() => {
    latest.current = handlers;
  });

  React.useEffect(() => {
    /** @param {KeyboardEvent} e */
    const onKeyDown = (e) => {
      // Autorepeat while the combo is held would flap a toggle on and off. Prod doesn't
      // guard this, but every binding here is a toggle, so one press = one flip.
      if (e.repeat) return;

      // Reject alt/ctrl on both arms so supersets never fire, and keep clear of prod's
      // ⌘⌥G (auto-tile, main.js:8312) if that lands later. No Ctrl fallback — see header.
      if (e.altKey || e.ctrlKey) return;

      // Symmetric: e.code match, preventDefault + stopPropagation so the chord never leaks
      // to the webview (devtools / browser Find) or a parent handler if one is ever added.
      if (e.metaKey && e.shiftKey && e.code === "KeyI") {
        e.preventDefault();
        e.stopPropagation();
        latest.current.onBroadcastToggle?.();
      } else if (e.metaKey && !e.shiftKey && e.code === "KeyG") {
        // ⌘G (no Shift) — see file-header: ⌘⇧G is WKWebView-swallowed; ⌘G is delivered.
        e.preventDefault();
        e.stopPropagation();
        latest.current.onMaximizeToggle?.();
      }
    };

    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);
}

export default useKeyboardShortcuts;
