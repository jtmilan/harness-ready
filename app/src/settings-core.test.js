// @vitest-environment happy-dom
// settings-core tests. trustedAddDecision is pure (node-safe) but the file also carries a
// happy-dom DOM builder, so the whole suite runs under happy-dom.
import { describe, it, expect, vi } from "vitest";
import { getByText, queryByText } from "@testing-library/dom";
import { trustedAddDecision, renderTrustedReposList, TRUSTED_ADD_CONFIRM_MS } from "./settings-core.js";

describe("trustedAddDecision — two-click add-confirm", () => {
  it("first click on a fresh state → arm (sets pending)", () => {
    const d = trustedAddDecision(null, 0, 1000, "/repo/a");
    expect(d.action).toBe("arm");
    expect(d.pending).toBe("/repo/a");
    expect(d.pendingAt).toBe(1000);
  });

  it("second click on the same path within 6s → confirm (disarms)", () => {
    const d = trustedAddDecision("/repo/a", 1000, 1000 + 5999, "/repo/a");
    expect(d.action).toBe("confirm");
    expect(d.pending).toBeNull();
    expect(d.pendingAt).toBe(0);
  });

  it("second click on the same path after 6s → arm again", () => {
    const d = trustedAddDecision("/repo/a", 1000, 1000 + TRUSTED_ADD_CONFIRM_MS, "/repo/a");
    expect(d.action).toBe("arm");
    expect(d.pending).toBe("/repo/a");
    expect(d.pendingAt).toBe(1000 + TRUSTED_ADD_CONFIRM_MS);
  });

  it("a different path within the window → arm the new path (does not confirm the old)", () => {
    const d = trustedAddDecision("/repo/a", 1000, 2000, "/repo/b");
    expect(d.action).toBe("arm");
    expect(d.pending).toBe("/repo/b");
    expect(d.pendingAt).toBe(2000);
  });
});

describe("renderTrustedReposList — trusted-repo list DOM builder", () => {
  it("empty / non-array → empty-state note", () => {
    const c = document.createElement("div");
    renderTrustedReposList(c, [], () => {});
    expect(getByText(c, "No trusted repositories yet.")).toBeTruthy();
    // non-array (older backend null) is treated as empty
    const c2 = document.createElement("div");
    renderTrustedReposList(c2, null, () => {});
    expect(getByText(c2, "No trusted repositories yet.")).toBeTruthy();
  });

  it("renders one row per repo with a labelled Remove button", () => {
    const c = document.createElement("div");
    renderTrustedReposList(c, ["/home/me/proj-a", "/home/me/proj-b"], () => {});
    // no empty-state
    expect(queryByText(c, "No trusted repositories yet.")).toBeNull();
    // two path labels
    expect(getByText(c, "/home/me/proj-a")).toBeTruthy();
    expect(getByText(c, "/home/me/proj-b")).toBeTruthy();
    // two Remove buttons, each aria-labelled with its path
    const removes = c.querySelectorAll("button");
    expect(removes).toHaveLength(2);
    expect(removes[0].getAttribute("aria-label")).toBe("Remove trusted repository /home/me/proj-a");
    expect(removes[1].getAttribute("aria-label")).toBe("Remove trusted repository /home/me/proj-b");
  });

  it("wires onRemove(path) to each Remove button", () => {
    const c = document.createElement("div");
    const onRemove = vi.fn();
    renderTrustedReposList(c, ["/x/y"], onRemove);
    c.querySelector("button").click();
    expect(onRemove).toHaveBeenCalledWith("/x/y");
  });

  it("re-render replaces prior content (no stale rows)", () => {
    const c = document.createElement("div");
    renderTrustedReposList(c, ["/one", "/two"], () => {});
    renderTrustedReposList(c, ["/three"], () => {});
    expect(queryByText(c, "/one")).toBeNull();
    expect(getByText(c, "/three")).toBeTruthy();
    expect(c.querySelectorAll("button")).toHaveLength(1);
  });
});
