// Agent Teams — svg-icon-core: build an inline Lucide icon node.
//
// Renders <svg class="icon"><use href="#id"/></svg> via DOM — an SVG element needs its
// namespace (createElementNS), and the <use> href references a <symbol> in the sprite.
// Building via the DOM (not innerHTML) keeps icon markup off the HTML string path.
//
// Pure: takes a symbol id, returns a fresh detached node. No module-level state, no I/O.
// Extracted from main.js so it can be DOM-tested in isolation — main.js imports svgIcon
// from here (single source of truth), so runtime behavior is unchanged.
export function svgIcon(symbolId) {
  const NS = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(NS, "svg");
  svg.setAttribute("class", "icon");
  svg.setAttribute("aria-hidden", "true");
  const use = document.createElementNS(NS, "use");
  use.setAttribute("href", "#" + symbolId);
  svg.appendChild(use);
  return svg;
}
