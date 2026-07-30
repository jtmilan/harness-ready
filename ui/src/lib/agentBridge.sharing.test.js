import { describe, it, expect } from "vitest";
import { bridge } from "@/lib/agentBridge";

// F-OBS-4 (M2): Home.jsx awaits bridge.setWorkspaceSharing / polls fetchSharingStates
// unconditionally. The web-preview mock must provide interface parity with
// TauriAgentBridge (same precedent as resumeAgents / broadcastRaw), or the first
// sharing toggle in the hosted preview TypeErrors. Sharing stays backend-authoritative:
// the mock no-ops keep every workspace ISOLATED (safe default).
describe("mock bridge sharing parity (F-OBS-4 / M2)", () => {
  it("exposes the sharing methods Home calls unconditionally", () => {
    expect(typeof bridge.setWorkspaceSharing).toBe("function");
    expect(typeof bridge.getSharing).toBe("function");
    expect(typeof bridge.fetchSharingStates).toBe("function");
  });

  it("setWorkspaceSharing resolves without throwing", async () => {
    await expect(bridge.setWorkspaceSharing("ws-1", true)).resolves.toBeUndefined();
  });

  it("sharing state reads as empty/isolated, never faked as shared", async () => {
    expect(bridge.getSharing()).toEqual({});
    await expect(bridge.fetchSharingStates()).resolves.toEqual({});
  });
});
