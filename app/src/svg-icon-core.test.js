// @vitest-environment happy-dom
import { describe, it, expect } from "vitest";
import { svgIcon } from "./svg-icon-core.js";

const SVG_NS = "http://www.w3.org/2000/svg";

describe("svgIcon — inline sprite icon builder (DOM)", () => {
  it("builds an <svg class=icon aria-hidden> wrapping a <use href=#id>", () => {
    const node = svgIcon("i-x");
    expect(node.namespaceURI).toBe(SVG_NS);
    expect(node.tagName.toLowerCase()).toBe("svg");
    expect(node.getAttribute("class")).toBe("icon");
    expect(node.getAttribute("aria-hidden")).toBe("true");
    const use = node.firstChild;
    expect(use.namespaceURI).toBe(SVG_NS);
    expect(use.tagName.toLowerCase()).toBe("use");
    expect(use.getAttribute("href")).toBe("#i-x");
  });

  it("returns a fresh detached node each call (no shared state)", () => {
    const a = svgIcon("i-plus");
    const b = svgIcon("i-plus");
    expect(a).not.toBe(b);
    expect(a.parentNode).toBeNull();
    expect(a.firstChild.getAttribute("href")).toBe("#i-plus");
  });
});
