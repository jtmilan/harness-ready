// @vitest-environment happy-dom
import { describe, it, expect } from "vitest";
import { installTauriStub } from "./tauri-stub.js";

describe("installTauriStub — the DOM-test __TAURI__ mock", () => {
  it("plants window.__TAURI__.core.invoke driven by fixtures; unknown cmd rejects", async () => {
    const h = installTauriStub({
      invoke: { list_queue: [], echo: (args) => ({ got: args }) },
    });
    expect(await window.__TAURI__.core.invoke("list_queue")).toEqual([]);
    expect(await window.__TAURI__.core.invoke("echo", { a: 1 })).toEqual({ got: { a: 1 } });
    await expect(window.__TAURI__.core.invoke("nope")).rejects.toThrow(/no fixture/);
    expect(h.invoke).toHaveBeenCalledWith("list_queue");
    h.restore();
  });

  it("listen()/emit() drives registered callbacks; restore() tears the global down", async () => {
    const had = Object.prototype.hasOwnProperty.call(window, "__TAURI__");
    const h = installTauriStub({});
    const seen = [];
    await window.__TAURI__.event.listen("update-available", (e) => seen.push(e.payload));
    h.emit("update-available", { version: "9" });
    h.emit("other", { x: 1 }); // no listener → no-op
    expect(seen).toEqual([{ version: "9" }]);
    h.restore();
    if (!had) expect(Object.prototype.hasOwnProperty.call(window, "__TAURI__")).toBe(false);
  });
});
