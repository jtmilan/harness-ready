# Brief — Window-Mgmt Lane 3: integrate tiling + toolbar + pane-move into Home

## Topic/Intent
Serial integration lane: wire the two shipped building-block lanes (WM-1 `useTiling` BSP tiling hook;
WM-2 `LayoutToolbar` + `workspaceAssign`/`workspaceStore` cross-ws move) into the live UI by editing
ONLY `ui/src/pages/Home.jsx` + `ui/src/components/command/AgentPane.jsx`. Also author the pointer-based
pane-drag controller. Consume shipped modules, do not modify them.

## Best Practices Found
- EXTRACTED (WM-1 report `scratchpad/WM-1-tiling.report.md`): tiling host must be `position:relative`;
  panes render absolute at `rects[id]`; a pane absent from `rects` (single/focus) → `display:none` but
  KEEP MOUNTED (unmounting kills its xterm/PTY). Seam handles at `seam.rect`, `touch-action:none`,
  higher z-index than panes, `col-resize`/`row-resize` by `seam.dir`.
- EXTRACTED (WM-2 report `scratchpad/WM-2-workspace-move.report.md`): pass the active/first ws id as
  `defaultWsId` to `paneIdsForWorkspace` so unassigned panes stay visible (default-bucket rule).
  `moveAgentToWorkspace` writes localStorage only → caller must force a re-render to re-bucket.
- EXTRACTED (both reports + memory [[nexus-agents-ui-embed]]): drag is POINTER-based, NOT html5
  draggable — Tauri's webview intercepts native drag-and-drop. Mirror prod's pointer approach.
- INFERRED: keep xterm mount effect keyed on `[agent.id]` only so re-renders (drag/select) never
  remount the terminal.

## Industry-Standard Architecture/Patterns
- Headless layout hook owns geometry + tree state; the page is a thin consumer that positions panes
  and renders seam handles (separation of layout engine from presentation). Single source of truth for
  layout mode = the hook's `mode`/`setMode` (no duplicated React state to desync).
- Global `pointermove`/`pointerup` listeners attached on drag start, removed on drag end; 6px threshold
  distinguishes click from drag; `document.elementFromPoint` + `.closest('[data-ws-drop]' | '[data-pane-id]')`
  resolves the drop target (standard pointer-DnD hit-testing).

## Anti-Patterns (avoided)
- Unmounting hidden panes (kills PTY). → `display:none`, kept mounted.
- html5 `draggable`/native DnD (Tauri intercepts). → pointer events.
- Duplicating `mode` into a separate `layoutMode` state that can drift from the hook. → use hook `mode`.
- Editing shipped modules to make integration fit. → consume only; flag any needed tweak.

## Open Questions (AMBIGUOUS)
- Task step 4 asks for a `layoutMode` state initialised from the hook's `mode`. DEVIATION: I bind the
  toolbar directly to the hook's `mode`/`setMode` (the hook already persists per-wsId and reloads on ws
  switch) — a separate mirror state adds desync risk for no gain. Behaviour is identical + always synced.
- Merge affordance (step 7, optional): SKIPPED — `LayoutToolbar` (Lane 2, read-only) exposes no merge
  prop/button; adding merge UI belongs in that toolbar, not Home. Noted for coordinator.

## Sources
- scratchpad/WM-1-tiling.report.md, scratchpad/WM-2-workspace-move.report.md
- `ui/src/lib/layout/useTiling.js`, `ui/src/components/command/LayoutToolbar.jsx`,
  `ui/src/lib/workspaceAssign.js`, `ui/src/lib/workspaceStore.js`
- memory: nexus-agents-ui-embed

## Conformance
Conforms to both lanes' contracts (absolute panes at `rects`, kept-mounted hide, pointer-drag hit-test
via `data-ws-drop`/`data-pane-id`, default-bucket rule). One noted deviation: hook `mode` as single
source instead of a duplicate `layoutMode` state. Merge skipped (out-of-lane UI).
