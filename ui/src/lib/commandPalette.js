// commandPalette.js — pure command-filtering + active-index math for the ⌘K palette.
// Zero React / DOM dependencies; both functions are fully unit-tested in
// commandPalette.test.js. The shadcn (cmdk) palette path uses cmdk's own filter
// internally, but these helpers remain available for the dep-free path and any
// future palette variant that needs explicit control.

/**
 * @typedef {object} PaletteCommand
 * @property {string} id        Stable key (used as React key + dedup).
 * @property {string} label     Human-visible name shown in the list.
 * @property {string[]} [keywords] Extra search tokens (synonyms, aliases).
 * @property {string} [hint]    Right-aligned muted text (e.g. a keybinding).
 * @property {() => void} run   Side effect — the action the command performs.
 */

/**
 * Case-insensitive substring match over each command's `label` + optional
 * `keywords[]`. An empty or whitespace-only query returns every command in
 * its original (stable) order.
 *
 * @param {string} query   Raw input text (not yet trimmed).
 * @param {PaletteCommand[]} commands  Source list — never mutated.
 * @returns {PaletteCommand[]} Filtered list (may be empty).
 */
export function filterCommands(query, commands) {
  const q = (query || "").trim().toLowerCase();
  if (!q) return commands;
  return commands.filter((cmd) => {
    if (cmd.label.toLowerCase().includes(q)) return true;
    if (Array.isArray(cmd.keywords)) {
      for (const kw of cmd.keywords) {
        if (kw.toLowerCase().includes(q)) return true;
      }
    }
    return false;
  });
}

/**
 * Wrap-around active-index arithmetic for ArrowUp (direction = -1) and
 * ArrowDown (direction = +1) over a filtered list of `count` items.
 *
 * Edge cases (all verified in commandPalette.test.js):
 *   - count 0 → always returns -1 (no selection possible).
 *   - current -1 + down → 0 (first item).
 *   - current -1 + up   → count - 1 (last item).
 *   - at last + down    → 0 (wrap to top).
 *   - at 0 + up         → count - 1 (wrap to bottom).
 *
 * @param {-1|1} direction  -1 for ArrowUp, +1 for ArrowDown.
 * @param {number} count    Number of visible items (≥ 0).
 * @param {number} current  Current active index, or -1 when nothing is active.
 * @returns {number}        New active index, or -1 when count is 0.
 */
export function moveIndex(direction, count, current) {
  if (count <= 0) return -1;
  // No current selection: down → first, up → last.
  if (current < 0) {
    return direction > 0 ? 0 : count - 1;
  }
  const next = current + direction;
  if (next >= count) return 0;
  if (next < 0) return count - 1;
  return next;
}
