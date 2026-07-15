// useKeyboardShortcuts — the app's global keyboard shortcuts. This is the ONLY global keydown
// listener in the app: `components/ui/sidebar.jsx` has one that looks like a peer, but
// `SidebarProvider` is never mounted, so that code is dead. Do not extend it.
//
//   ⌘⇧I → onBroadcastToggle()   ⌘⇧G → onMaximizeToggle()
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

import * as React from "react";

/**
 * @typedef {object} ShortcutHandlers
 * @property {() => void} [onBroadcastToggle] ⌘⇧I — toggle broadcast-to-all-panes mode.
 * @property {() => void} [onMaximizeToggle]  ⌘⇧G — toggle zoom on the highlighted pane.
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
      // Require ⌘+⇧ exactly (macOS metaKey). Rejecting alt/ctrl keeps these from firing on
      // supersets, keeps ⌘⇧G clear of prod's ⌘⌥G (auto-tile, main.js:8312) if that lands
      // later, and deliberately refuses Ctrl+Shift — see file-header platform note.
      // Not a missing Windows/Linux port: the app does not ship those targets today.
      if (!e.metaKey || !e.shiftKey || e.altKey || e.ctrlKey) return;

      // Autorepeat while the combo is held would flap a toggle on and off. Prod doesn't
      // guard this, but every binding here is a toggle, so one press = one flip.
      if (e.repeat) return;

      // `e.code` (physical key), not `e.key`: macOS mangles `e.key` for modified letters —
      // ⌥G arrives as "©". Prod hit this and switched to `e.code === "KeyG"` (main.js:8312).
      // `e.code` is also layout-independent, so it survives a non-QWERTY keymap.
      if (e.code === "KeyI") {
        e.preventDefault();
        latest.current.onBroadcastToggle?.();
      } else if (e.code === "KeyG") {
        e.preventDefault();
        latest.current.onMaximizeToggle?.();
      }
    };

    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);
}

export default useKeyboardShortcuts;
