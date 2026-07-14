// Phase-14-T2 Tier B true-kanban — unit tests for the pure core.
//
// Tests the column-mapping and drag-logic helpers in kanban-core.js. These are
// PURE (no DOM, no IPC, no globals) so vitest runs them in Node with zero setup.
import { describe, it, expect } from "vitest";
import {
  mcpTaskCol,
  wireColToDisplay,
  displayColToWire,
  mcpEffectiveCol,
  buildCreatePayload,
  buildUpdatePayload,
} from "./kanban-core.js";

describe("mcpTaskCol — lifecycle → default board column", () => {
  it("created → Backlog (the agent-created default home)", () => {
    expect(mcpTaskCol("created")).toBe("Backlog");
  });

  it("assigned → Working (the agent has it in progress)", () => {
    expect(mcpTaskCol("assigned")).toBe("Working");
  });

  it("doing → Working (agent actively working)", () => {
    expect(mcpTaskCol("doing")).toBe("Working");
  });

  it("review → Needs you (human review required)", () => {
    expect(mcpTaskCol("review")).toBe("Needs you");
  });

  it("done → Done (terminal lifecycle stage)", () => {
    expect(mcpTaskCol("done")).toBe("Done");
  });

  it("unknown / empty lifecycle → Backlog (safe fallback)", () => {
    expect(mcpTaskCol("")).toBe("Backlog");
    expect(mcpTaskCol("bogus")).toBe("Backlog");
    expect(mcpTaskCol(null)).toBe("Backlog");
    expect(mcpTaskCol(undefined)).toBe("Backlog");
  });
});

describe("wireColToDisplay — wire form → display column", () => {
  it("maps all four valid wire forms to their display names", () => {
    expect(wireColToDisplay("backlog")).toBe("Backlog");
    expect(wireColToDisplay("doing")).toBe("Working");
    expect(wireColToDisplay("review")).toBe("Needs you");
    expect(wireColToDisplay("done")).toBe("Done");
  });

  it("returns null for unknown wire forms (AC7: status/state are never valid)", () => {
    expect(wireColToDisplay("status")).toBeNull();
    expect(wireColToDisplay("state")).toBeNull();
    expect(wireColToDisplay("working")).toBeNull(); // machine-state word, not a wire col
    expect(wireColToDisplay("")).toBeNull();
  });
});

describe("displayColToWire — display column → wire form (for CRUD commands)", () => {
  it("maps all four display columns to their wire forms", () => {
    expect(displayColToWire("Backlog")).toBe("backlog");
    expect(displayColToWire("Working")).toBe("doing");
    expect(displayColToWire("Needs you")).toBe("review");
    expect(displayColToWire("Done")).toBe("done");
  });

  it("falls back to 'backlog' for unknown display names (safe default)", () => {
    expect(displayColToWire("Scheduled")).toBe("backlog"); // machine-state col, not a Task col
    expect(displayColToWire("")).toBe("backlog");
    expect(displayColToWire(null)).toBe("backlog");
  });
});

describe("mcpEffectiveCol — operator-store override wins over lifecycle default", () => {
  it("no override → lifecycle-derived default", () => {
    expect(mcpEffectiveCol("created", null)).toBe("Backlog");
    expect(mcpEffectiveCol("doing", undefined)).toBe("Working");
    expect(mcpEffectiveCol("review", null)).toBe("Needs you");
  });

  it("operator-store override (column = 'doing') overrides a 'created' lifecycle", () => {
    // An agent-created task dragged from Backlog → Working by the operator persists
    // as column='doing' in the operator store. Effective col = Working, not Backlog.
    const override = { id: "task-123", column: "doing" };
    expect(mcpEffectiveCol("created", override)).toBe("Working");
  });

  it("operator-store override (column = 'review') overrides a 'doing' lifecycle", () => {
    const override = { id: "task-456", column: "review" };
    expect(mcpEffectiveCol("doing", override)).toBe("Needs you");
  });

  it("operator-store override (column = 'done') overrides a 'review' lifecycle", () => {
    const override = { id: "task-789", column: "done" };
    expect(mcpEffectiveCol("review", override)).toBe("Done");
  });

  it("operator-store override (column = 'backlog') overrides a 'doing' lifecycle", () => {
    // Operator moved a 'doing' task BACK to backlog (e.g. deprioritized).
    const override = { id: "task-abc", column: "backlog" };
    expect(mcpEffectiveCol("doing", override)).toBe("Backlog");
  });

  it("override with an unknown column falls back to lifecycle default", () => {
    // A corrupt / future override wire form the display map doesn't know → lifecycle wins.
    const override = { id: "task-xyz", column: "unknown-future-col" };
    expect(mcpEffectiveCol("doing", override)).toBe("Working"); // lifecycle fallback
  });

  it("override with absent column falls back to lifecycle default", () => {
    const override = { id: "task-xyz" }; // no column field
    expect(mcpEffectiveCol("created", override)).toBe("Backlog");
  });
});

describe("buildCreatePayload — first-time column move payload for create_task_kanban", () => {
  it("uses the task id and title from the MCP task, maps the display column to wire form", () => {
    const mcpTask = { id: "task-001", title: "wire the widget", lifecycle: "created" };
    const payload = buildCreatePayload(mcpTask, "Working");
    expect(payload.id).toBe("task-001");
    expect(payload.title).toBe("wire the widget");
    expect(payload.column).toBe("doing"); // Working → doing wire form
    expect(payload.order).toBeNull();
  });

  it("falls back to empty string when the MCP task has no title (genesis-less scenario)", () => {
    const mcpTask = { id: "task-002", lifecycle: "created" }; // no title
    const payload = buildCreatePayload(mcpTask, "Backlog");
    expect(payload.title).toBe("");
    expect(payload.column).toBe("backlog");
  });

  it("maps all four display columns correctly in the payload", () => {
    const task = { id: "t", title: "x", lifecycle: "created" };
    expect(buildCreatePayload(task, "Backlog").column).toBe("backlog");
    expect(buildCreatePayload(task, "Working").column).toBe("doing");
    expect(buildCreatePayload(task, "Needs you").column).toBe("review");
    expect(buildCreatePayload(task, "Done").column).toBe("done");
  });
});

describe("buildUpdatePayload — subsequent column move payload for update_task_kanban", () => {
  it("only id + column (AC7: no status/state field)", () => {
    const payload = buildUpdatePayload("task-001", "Needs you");
    expect(payload.id).toBe("task-001");
    expect(payload.column).toBe("review");
    // The payload must NOT carry status or state (AC7 structural guard).
    expect("status" in payload).toBe(false);
    expect("state" in payload).toBe(false);
  });

  it("maps all four display columns correctly in the payload", () => {
    expect(buildUpdatePayload("t", "Backlog").column).toBe("backlog");
    expect(buildUpdatePayload("t", "Working").column).toBe("doing");
    expect(buildUpdatePayload("t", "Needs you").column).toBe("review");
    expect(buildUpdatePayload("t", "Done").column).toBe("done");
  });
});
