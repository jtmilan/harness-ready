// Mock bridge parity for K1 + K4 (p7 owns the implementation).
// MockAgentBridge is module-private; only `bridge` is exported. When K1 lands,
// `bridge.spawnAgents` (web-preview path) must return Promise<string[]>. Until
// then these stay it.todo so the gate stays green without weakening assertions.
import { describe, it } from "vitest";

describe("MockAgentBridge.spawnAgents — K1 + K4 (pinned; p7)", () => {
  it.todo(
    "K1: spawnAgents resolves to minted ids in config order (Promise<string[]>)",
  );
  it.todo(
    "K1: a failed config is excluded from the returned id list",
  );
  it.todo(
    "K4: unmapped kind refused loudly — never silent bash fallback",
  );
});
