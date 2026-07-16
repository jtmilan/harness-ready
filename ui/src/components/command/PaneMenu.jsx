import React from "react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

// Pane-head kebab menu.
//
// The behaviour bar is prod's hand-rolled popover (agent-teams/app/src/main.js:302-349):
// Escape closes (:295), ArrowUp/Down cycle the items with wrapping (:270), the menu mounts
// outside the grid so overflow can't clip it (:301), and it flips above the trigger when it
// would overhang the viewport (:352). Radix's DropdownMenu ships every one of those, so this is
// a SKIN over the repo's existing `components/ui/dropdown-menu` rather than a second hand-rolled
// menu. Radix adds typeahead and Home/End on top.
//
// Display-only: no item performs its own side effect. Every choice leaves via `onAction`, which
// is what keeps AgentPane presentational (BRIEF C2) — the clipboard writes, the tauri
// `close_workspace`, and the pane->workspace re-assignment all belong to the container.

// ---- CRT skin.
// `components/ui/*` is themed for shadcn tokens, and this app never sets the `dark` class, so
// `bg-popover` would resolve to WHITE (index.css:13) against the pane's cyan-on-near-black. Every
// colour token is therefore overridden here; `cn` runs tailwind-merge, so the last conflicting
// utility wins over the primitive's default. `rounded-none` matches the square pane chrome.
const CONTENT_CLS =
  "min-w-[13rem] rounded-none border-cyan-800/70 bg-[#0C1720] p-1 text-cyan-300 shadow-[0_0_16px_rgba(0,229,255,0.18)]";
const ITEM_CLS =
  "cursor-pointer rounded-none px-2 py-1.5 font-mono text-[11px] tracking-wider text-cyan-300 " +
  "focus:bg-cyan-300/15 focus:text-cyan-100 data-[disabled]:opacity-40";
const DANGER_CLS =
  "cursor-pointer rounded-none px-2 py-1.5 font-mono text-[11px] tracking-wider text-red-400 " +
  "focus:bg-red-500/20 focus:text-red-300";

// Fixed-width glyph gutter so the six labels align regardless of glyph width (prod's .ph-menu-ico).
// Inherits the row's colour so it dims with a disabled row and reddens on the danger row.
function Glyph({ children }) {
  return (
    <span aria-hidden="true" className="w-3.5 shrink-0 text-center opacity-70">
      {children}
    </span>
  );
}

/**
 * @param {object}   props
 * @param {boolean}  props.hasBranch    pane resolved a git branch — gates "Copy branch"
 * @param {{id: string, name: string}[]} props.workspaces  move targets, already filtered by the
 *   container (see AgentPane's `workspaces` prop). Empty ⇒ the submenu is disabled.
 * @param {(action: string, payload?: object) => void} props.onAction
 */
export default function PaneMenu({ hasBranch = false, workspaces = [], onAction }) {
  // Radix returns focus to the trigger when the menu closes — right for five of the six items.
  // "Rename" hands focus to the header's inline editor, and the restore fires AFTER onSelect, so
  // without this it drags focus straight back out of the field the user is meant to type into.
  const handsOffFocusRef = React.useRef(false);
  const fire = (action, payload) => {
    handsOffFocusRef.current = action === "rename";
    onAction && onAction(action, payload);
  };

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          // The pane head starts a drag on pointerdown (AgentPane) and selects the pane on click.
          // Stop both from reaching it. stopPropagation ONLY — preventDefault would leave Radix's
          // own composed pointerdown unable to open the menu.
          onPointerDown={(e) => e.stopPropagation()}
          onClick={(e) => e.stopPropagation()}
          title="Pane menu"
          aria-label="Pane menu"
          className="flex h-5 w-4 shrink-0 items-center justify-center text-sm leading-none text-cyan-600 transition-colors hover:bg-cyan-300/10 hover:text-cyan-300 data-[state=open]:bg-cyan-300/10 data-[state=open]:text-cyan-300"
        >
          {/* Decorative glyph — aria-label names the control; without aria-hidden SRs announce both. */}
          <span aria-hidden="true">⋮</span>
        </button>
      </DropdownMenuTrigger>

      {/* align="end" keeps the menu's right edge on the kebab, mirroring prod's
          `left = r.right - mw` anchor (main.js:352). `loop` is NOT a Radix default: without it
          arrows dead-end at the first/last row, where prod's nav wraps (main.js:270). */}
      <DropdownMenuContent
        align="end"
        sideOffset={4}
        loop
        onCloseAutoFocus={(e) => {
          if (!handsOffFocusRef.current) return; // every other item: let focus fall back to ⋮
          handsOffFocusRef.current = false;
          e.preventDefault();
        }}
        className={CONTENT_CLS}
      >
        <DropdownMenuItem className={ITEM_CLS} onSelect={() => fire("rename")}>
          <Glyph>✎</Glyph>
          Rename
        </DropdownMenuItem>

        <DropdownMenuItem className={ITEM_CLS} onSelect={() => fire("maximize")}>
          <Glyph>⤢</Glyph>
          Maximize
        </DropdownMenuItem>

        <DropdownMenuItem className={ITEM_CLS} onSelect={() => fire("copy-id")}>
          <Glyph>⧉</Glyph>
          Copy id
        </DropdownMenuItem>

        {/* Disabled, not hidden, when the pane has no branch yet: `agent.branch` is "" until the
            backend resolves the worktree (tauriAgentBridge.js:211), and copying "" is a no-op the
            user can't tell from a broken menu. Prod omits the item entirely (main.js:330); a
            stable six-item menu that greys one row reads better than one that reflows. */}
        <DropdownMenuItem className={ITEM_CLS} disabled={!hasBranch} onSelect={() => fire("copy-branch")}>
          <Glyph>⎇</Glyph>
          Copy branch
        </DropdownMenuItem>

        <DropdownMenuSub>
          {/* SubTrigger renders its own ChevronRight — that is the ▸ affordance. */}
          <DropdownMenuSubTrigger className={ITEM_CLS} disabled={!workspaces.length}>
            <Glyph>⇄</Glyph>
            Move to workspace
          </DropdownMenuSubTrigger>
          <DropdownMenuSubContent loop className={CONTENT_CLS}>
            {workspaces.map((ws) => (
              <DropdownMenuItem
                key={ws.id}
                className={ITEM_CLS}
                onSelect={() => fire("move-to-ws", { wsId: ws.id })}
              >
                {ws.name}
              </DropdownMenuItem>
            ))}
          </DropdownMenuSubContent>
        </DropdownMenuSub>

        <DropdownMenuSeparator className="bg-cyan-800/70" />

        <DropdownMenuItem className={DANGER_CLS} onSelect={() => fire("close")}>
          <Glyph>✕</Glyph>
          Close
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
