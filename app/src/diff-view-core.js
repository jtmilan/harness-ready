// Agent Teams — diff-view-core: render a unified-diff string into a DOM node.
//
// Pure DOM builders, no module-level state, no I/O:
//   diffLineClass(line)      → the .diff-* CSS class for one unified-diff line
//   renderUnifiedDiff(text)  → a .diff-body <div> of one .diff-line row per source line
//
// Extracted from main.js so they can be DOM-tested in isolation — main.js imports them
// from here (single source of truth), so runtime behavior is unchanged.

// Classify one unified-diff line for coloring. Header/meta lines first (so a "+++"/"---"
// file header isn't mistaken for an added/removed line), then hunk, then +/- content.
export function diffLineClass(line) {
  if (line.startsWith("+++") || line.startsWith("---")) return "diff-meta";
  if (line.startsWith("@@")) return "diff-hunk";
  if (line.startsWith("diff ") || line.startsWith("index ") || line.startsWith("new file") || line.startsWith("deleted file") || line.startsWith("rename ") || line.startsWith("similarity ")) return "diff-meta";
  const c = line.charAt(0);
  if (c === "+") return "diff-added";
  if (c === "-") return "diff-removed";
  return "diff-context";
}

// Build the scrollable diff body. Empty/whitespace-only text → a single .diff-empty
// placeholder. Otherwise one .diff-line row per source line; a blank line becomes a
// single space so the row keeps its height.
export function renderUnifiedDiff(text) {
  const body = document.createElement("div");
  body.className = "diff-body";
  if (!text || !text.trim()) {
    const empty = document.createElement("div");
    empty.className = "diff-empty";
    empty.textContent = "No uncommitted changes in this worktree.";
    body.appendChild(empty);
    return body;
  }
  for (const line of text.split("\n")) {
    const row = document.createElement("div");
    row.className = "diff-line " + diffLineClass(line);
    row.textContent = line === "" ? " " : line;
    body.appendChild(row);
  }
  return body;
}
