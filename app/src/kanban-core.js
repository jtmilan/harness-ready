// Phase-14-T2 Tier B true-kanban — pure core (no DOM, no globals, no IPC).
//
// Extracted from main.js so vitest can import and exercise the pure logic
// (column mapping, effective-column resolution, column-wire parsing) without a
// browser context. Everything here must be PURE (no side effects, no module-level
// mutable state) so the tests can be deterministic.
//
// Consumers: main.js imports mcpTaskCol / mcpEffectiveCol / kanbanColWire for
// rendering; kanban-core.test.js exercises them directly.

// lifecycle (created|assigned|doing|review|done) → default board column.
// When a task has a durable operator-store column override, `mcpEffectiveCol`
// wins over this.
export function mcpTaskCol(lifecycle) {
  switch (lifecycle) {
    case "assigned":
    case "doing": return "Working";
    case "review": return "Needs you";
    case "done": return "Done";
    default: return "Backlog"; // created + any unknown stage
  }
}

// Map the wire column form (backlog/doing/review/done from list_tasks_kanban) to
// a display column name (the keys in BOARD_COLS).
export function wireColToDisplay(wireCol) {
  switch (wireCol) {
    case "backlog": return "Backlog";
    case "doing": return "Working";
    case "review": return "Needs you";
    case "done": return "Done";
    default: return null; // unknown wire form
  }
}

// Map a display column name back to the wire form accepted by update_task_kanban.
// Returns "backlog" for any unknown display name (safe fallback).
export function displayColToWire(displayCol) {
  switch (displayCol) {
    case "Backlog": return "backlog";
    case "Working": return "doing";
    case "Needs you": return "review";
    case "Done": return "done";
    default: return "backlog";
  }
}

// Effective board column for an MCP task: the operator-store column override wins
// over the lifecycle-derived default. `operatorOverride` is the TaskRow from
// `list_tasks_kanban` (or null/undefined if no operator override exists).
export function mcpEffectiveCol(lifecycle, operatorOverride) {
  if (operatorOverride && operatorOverride.column) {
    const display = wireColToDisplay(operatorOverride.column);
    if (display) return display;
  }
  return mcpTaskCol(lifecycle);
}

// Build the create_task_kanban payload for a first-time MCP task column move.
// The id is passed through (the MCP id is stable and app-minted).
export function buildCreatePayload(mcpTask, targetDisplayCol) {
  return {
    id: mcpTask.id,
    title: mcpTask.title || "",
    column: displayColToWire(targetDisplayCol),
    order: null,
  };
}

// Build the update_task_kanban payload for a subsequent MCP task column move
// (the task already has an operator-store entry).
export function buildUpdatePayload(taskId, targetDisplayCol) {
  return {
    id: taskId,
    column: displayColToWire(targetDisplayCol),
  };
}
