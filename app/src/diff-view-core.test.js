// @vitest-environment happy-dom
import { describe, it, expect } from "vitest";
import { getByText } from "@testing-library/dom";
import { diffLineClass, renderUnifiedDiff } from "./diff-view-core.js";

describe("diffLineClass — unified-diff line classification", () => {
  it("classifies meta/hunk headers before +/- content", () => {
    expect(diffLineClass("+++ b/file")).toBe("diff-meta"); // file header, NOT added
    expect(diffLineClass("--- a/file")).toBe("diff-meta"); // file header, NOT removed
    expect(diffLineClass("@@ -1,2 +1,3 @@")).toBe("diff-hunk");
    expect(diffLineClass("diff --git a/x b/x")).toBe("diff-meta");
    expect(diffLineClass("index abc..def 100644")).toBe("diff-meta");
    expect(diffLineClass("new file mode 100644")).toBe("diff-meta");
    expect(diffLineClass("rename from x")).toBe("diff-meta");
  });
  it("classifies +/- content and context", () => {
    expect(diffLineClass("+added")).toBe("diff-added");
    expect(diffLineClass("-removed")).toBe("diff-removed");
    expect(diffLineClass(" context")).toBe("diff-context");
    expect(diffLineClass("")).toBe("diff-context");
  });
});

describe("renderUnifiedDiff — DOM builder", () => {
  it("empty/whitespace text → a single .diff-empty placeholder", () => {
    const body = renderUnifiedDiff("");
    expect(body.className).toBe("diff-body");
    expect(body.children).toHaveLength(1);
    expect(body.firstChild.className).toBe("diff-empty");
    // testing-library query over the produced node
    expect(getByText(body, /No uncommitted changes/)).toBeTruthy();
    expect(renderUnifiedDiff("   \n  ").firstChild.className).toBe("diff-empty");
  });

  it("renders one .diff-line row per source line, class-tagged, blank→space", () => {
    const body = renderUnifiedDiff("@@ -1 +1 @@\n+new\n-old\n ctx\n");
    const rows = [...body.children];
    // 5 lines (trailing \n yields a final empty row → single space)
    expect(rows).toHaveLength(5);
    expect(rows[0].className).toBe("diff-line diff-hunk");
    expect(rows[1].className).toBe("diff-line diff-added");
    expect(rows[1].textContent).toBe("+new");
    expect(rows[2].className).toBe("diff-line diff-removed");
    expect(rows[3].className).toBe("diff-line diff-context");
    expect(rows[4].textContent).toBe(" "); // blank line keeps row height
  });
});
