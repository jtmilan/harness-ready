// Tests for the Phase-1 layout-wizard PURE model (wizard-core.js).
//
// Spec under test: .paul/analysis/preset-wizard/PHASE1-CONTRACT.md — the
// "wizard-core.js — EXACT pure API" section. Tested to the CONTRACT only: the
// implementation is written in parallel (facet C1), so nothing here assumes
// behavior the contract does not pin. Pure logic → vitest default 'node' env,
// no DOM.
import { describe, expect, it } from "vitest";
import {
  DEFAULT_HARNESS,
  DEFAULT_ROLE,
  TILE_COUNTS,
  createInitialState,
  setStep,
  goNext,
  goBack,
  setName,
  setFolder,
  setSeedPrompt,
  setCount,
  setPaneHarness,
  setPaneRole,
  setPaneModel,
  applyPreset,
  openWithoutAI,
  canAdvance,
  isLastStep,
  toCreateArgs,
  gridShape,
  overflowCount,
} from "./wizard-core.js";
import { KNOWN_HARNESSES, KNOWN_ROLES } from "./presets.js";

// A normalized Preset literal (presets.js shape: {id,name,count,harnesses,folder?,seedPrompt?}).
// INVARIANT count === harnesses.length is honored — applyPreset trusts preset.count
// verbatim, so a malformed fake would break the invariant by the contract's own code.
function fakePreset(over = {}) {
  return { id: "preset-fake", name: "Fake Mix", count: 2, harnesses: ["claude", "cursor"], ...over };
}

// Deep structural snapshot, for proving a reducer did not mutate its input.
const snap = (o) => JSON.parse(JSON.stringify(o));

describe("createInitialState", () => {
  it("has the pinned shape: step 1, harnesses length === count, activePresetId null", () => {
    const s = createInitialState();
    expect(s.step).toBe(1);
    expect(s.count).toBe(1);
    expect(s.harnesses).toHaveLength(s.count);
    expect(s.harnesses).toEqual([DEFAULT_HARNESS]);
    expect(s.roles).toHaveLength(s.count);
    expect(s.roles).toEqual([DEFAULT_ROLE]);
    expect(s.models).toHaveLength(s.count);
    expect(s.models).toEqual([""]); // model-at-spawn: "" = account default
    expect(s.activePresetId).toBeNull();
    expect(s.seedPrompt).toBe("");
    expect(s.cap).toBeNull();   // Scheduler cap snapshot — null when unknown (no opts)
    expect(s.working).toBe(0);  // live working-count snapshot — 0 default
    // exact state shape — no stray keys (defaultHarness is an opt, NOT a state field)
    expect(Object.keys(s).sort()).toEqual(
      ["activePresetId", "cap", "count", "folder", "harnesses", "models", "name", "roles", "seedPrompt", "step", "working"],
    );
    expect("defaultHarness" in s).toBe(false);
  });

  it("respects folder/name/count/defaultHarness opts and keeps the length invariant", () => {
    const s = createInitialState({ folder: "/repo/x", name: "Proj", count: 3, defaultHarness: "cursor" });
    expect(s.folder).toBe("/repo/x");
    expect(s.name).toBe("Proj");
    expect(s.count).toBe(3);
    expect(s.harnesses).toEqual(["cursor", "cursor", "cursor"]);
    expect(s.harnesses).toHaveLength(s.count);
  });

  it("exports the pinned constants", () => {
    expect(DEFAULT_HARNESS).toBe("claude");
    expect(TILE_COUNTS).toEqual([1, 2, 4, 6, 9, 10, 12]); // ruling A: keep 9
    // KNOWN_HARNESSES is the SSOT the contract sources harness ids from
    expect(KNOWN_HARNESSES).toContain(DEFAULT_HARNESS);
  });
});

describe("purity — reducers return a new object and never mutate the input", () => {
  it("state-changing reducers do not mutate their input", () => {
    const base = createInitialState({ folder: "/r", name: "N", count: 2 });
    base.activePresetId = "preset-x"; // so we can also see nulling happen elsewhere
    const before = snap(base);

    const calls = [
      (s) => setStep(s, 3),
      (s) => goNext(s),
      (s) => goBack(s),
      (s) => setName(s, "Other"),
      (s) => setFolder(s, "/other"),
      (s) => setSeedPrompt(s, "go"),
      (s) => setCount(s, 4),
      (s) => setPaneHarness(s, 0, "bash"),
      (s) => applyPreset(s, fakePreset()),
      (s) => openWithoutAI(s),
    ];
    for (const fn of calls) {
      const out = fn(base);
      expect(out).not.toBe(base);          // new top-level object (immutable reducer)
      expect(snap(base)).toEqual(before);  // input deeply unchanged (no mutation)
    }
    // reducers that rebuild the array must not alias the input's array (would let a
    // later mutation leak back). Only assert this for reducers the contract says
    // produce a fresh harnesses[]; spread-only reducers (setName, etc.) may share it.
    for (const fn of [
      (s) => setCount(s, 4),
      (s) => setPaneHarness(s, 0, "bash"),
      (s) => applyPreset(s, fakePreset()),
      (s) => openWithoutAI(s),
    ]) {
      expect(fn(base).harnesses).not.toBe(base.harnesses);
      expect(snap(base)).toEqual(before);
    }
  });
});

describe("scalar setters — set their field, leave the rest", () => {
  it("setName / setFolder / setSeedPrompt each set only their own field", () => {
    const s = createInitialState({ folder: "/a", name: "A" });
    expect(setName(s, "B").name).toBe("B");
    expect(setFolder(s, "/b").folder).toBe("/b");
    expect(setSeedPrompt(s, "go").seedPrompt).toBe("go");
    expect(setName(s, "B").folder).toBe("/a"); // isolation: other fields untouched
  });
});

describe("setCount — invariant + padding + preset desync", () => {
  it("grows by padding (length === count) and shrinks by truncation", () => {
    const grown = setCount(createInitialState({ count: 1 }), 4);
    expect(grown.count).toBe(4);
    expect(grown.harnesses).toHaveLength(4);

    const shrunk = setCount(createInitialState({ count: 6 }), 2);
    expect(shrunk.count).toBe(2);
    expect(shrunk.harnesses).toHaveLength(2);
  });

  it("pads new slots with the LAST existing harness", () => {
    const s = createInitialState({ count: 2 });
    const s2 = setPaneHarness(s, 1, "cursor"); // -> ["claude","cursor"]
    const grown = setCount(s2, 4);
    expect(grown.harnesses).toEqual(["claude", "cursor", "cursor", "cursor"]);
  });

  it("pads with DEFAULT_HARNESS when there is no existing harness", () => {
    // normal flow never empties harnesses; hand-craft the empty edge to hit the branch
    const empty = { ...createInitialState({ count: 1 }), count: 0, harnesses: [] };
    const grown = setCount(empty, 3);
    expect(grown.harnesses).toEqual([DEFAULT_HARNESS, DEFAULT_HARNESS, DEFAULT_HARNESS]);
  });

  it("nulls activePresetId on a real count change (count desyncs the preset)", () => {
    const applied = applyPreset(createInitialState({ count: 1 }), fakePreset());
    expect(applied.activePresetId).toBe("preset-fake");
    const changed = setCount(applied, 5);
    expect(changed.activePresetId).toBeNull();
  });

  it("clamps n to a minimum of 1", () => {
    const s = setCount(createInitialState({ count: 3 }), 0);
    expect(s.count).toBe(1);
    expect(s.harnesses).toHaveLength(1);
  });
});

describe("setPaneHarness — single index, range guard, desync", () => {
  it("sets exactly one index and nulls activePresetId", () => {
    const applied = applyPreset(createInitialState({ count: 1 }), fakePreset()); // count 2
    const out = setPaneHarness(applied, 1, "bash");
    expect(out.harnesses).toEqual(["claude", "bash"]);
    expect(out.activePresetId).toBeNull();
    expect(out.harnesses).toHaveLength(out.count);
  });

  it("is a no-op for an out-of-range index (harnesses unchanged)", () => {
    // start with activePresetId already null so this holds under both contract readings
    const s = createInitialState({ count: 2 }); // activePresetId null
    expect(setPaneHarness(s, 5, "bash").harnesses).toEqual(s.harnesses);
    expect(setPaneHarness(s, -1, "bash").harnesses).toEqual(s.harnesses);
  });
});

describe("applyPreset — copy layout, ruling D (wizard wins, preset pre-fills)", () => {
  it("copies count + harnesses from the preset and sets activePresetId", () => {
    const out = applyPreset(createInitialState({ count: 1 }), fakePreset());
    expect(out.count).toBe(2);
    expect(out.harnesses).toEqual(["claude", "cursor"]);
    expect(out.harnesses).toHaveLength(out.count); // invariant
    expect(out.activePresetId).toBe("preset-fake");
  });

  it("keeps the user's non-blank folder/name/seedPrompt over the preset's", () => {
    const base = { ...createInitialState({ folder: "/mine", name: "MyName" }), seedPrompt: "my prompt" };
    const out = applyPreset(base, fakePreset({ folder: "/preset", name: "PName", seedPrompt: "preset prompt" }));
    expect(out.folder).toBe("/mine");
    expect(out.name).toBe("MyName");
    expect(out.seedPrompt).toBe("my prompt");
  });

  it("falls back to the preset's folder/name/seedPrompt when the user's are blank", () => {
    const base = createInitialState({ folder: "", name: "" }); // seedPrompt "" too
    const out = applyPreset(base, fakePreset({ folder: "/preset", name: "PName", seedPrompt: "preset prompt" }));
    expect(out.folder).toBe("/preset");
    expect(out.name).toBe("PName");
    expect(out.seedPrompt).toBe("preset prompt");
  });

  it("uses empty-string fallbacks when neither user nor preset supply folder/name", () => {
    const base = createInitialState(); // blank folder/name
    const out = applyPreset(base, fakePreset()); // fake has no folder/name override
    expect(out.folder).toBe("");
    expect(out.name).toBe("Fake Mix"); // preset.name present on the fake
  });
});

describe("openWithoutAI", () => {
  it("makes every harness 'bash' and preserves length, nulling the preset", () => {
    const applied = applyPreset(createInitialState({ count: 1 }), fakePreset()); // count 2
    const out = openWithoutAI(applied);
    expect(out.harnesses).toEqual(["bash", "bash"]);
    expect(out.harnesses).toHaveLength(out.count); // invariant preserved
    expect(out.count).toBe(2);
    expect(out.activePresetId).toBeNull();
  });
});

describe("canAdvance", () => {
  it("step 1 with a blank folder is blocked with a folder reason", () => {
    const r = canAdvance(createInitialState({ folder: "" }));
    expect(r.ok).toBe(false);
    expect(r.reason).toMatch(/folder/i);
  });

  it("step 1 with a folder (incl. whitespace-trim) advances", () => {
    expect(canAdvance(createInitialState({ folder: "/repo" })).ok).toBe(true);
    expect(canAdvance(createInitialState({ folder: "   " })).ok).toBe(false); // blank after trim
  });

  it("steps 2 and 3 always advance (no folder gate)", () => {
    const blank = createInitialState({ folder: "" });
    expect(canAdvance(setStep(blank, 2)).ok).toBe(true);
    expect(canAdvance(setStep(blank, 3)).ok).toBe(true);
  });
});

describe("step navigation — clamp to 1..3", () => {
  it("goNext / goBack clamp at the bounds", () => {
    const s1 = createInitialState();
    expect(goBack(s1).step).toBe(1);           // already at 1
    expect(goNext(s1).step).toBe(2);
    expect(goNext(goNext(s1)).step).toBe(3);
    expect(goNext(goNext(goNext(s1))).step).toBe(3); // clamped at 3
  });

  it("setStep clamps out-of-range values", () => {
    const s = createInitialState();
    expect(setStep(s, 0).step).toBe(1);
    expect(setStep(s, 99).step).toBe(3);
    expect(setStep(s, 2).step).toBe(2);
  });

  it("isLastStep is true only on step 3", () => {
    expect(isLastStep(setStep(createInitialState(), 3))).toBe(true);
    expect(isLastStep(setStep(createInitialState(), 2))).toBe(false);
  });
});

describe("toCreateArgs", () => {
  it("emits repo + harness + harnesses + count + prompt, with NO color key", () => {
    const base = { ...createInitialState({ folder: "/repo/app", name: "App", count: 2 }), seedPrompt: "hi" };
    const withPane = setPaneHarness(base, 1, "cursor"); // harnesses ["claude","cursor"]
    const args = toCreateArgs(withPane);
    expect(args.repo).toBe("/repo/app");
    expect(args.harness).toBe("claude");      // harnesses[0]
    expect(args.harnesses).toEqual(["claude", "cursor"]);
    expect(args.count).toBe(2);
    expect(args.prompt).toBe("hi");
    expect("color" in args).toBe(false);      // main.js adds color, not core
  });

  it("name falls back: explicit name → folder basename → 'workspace'", () => {
    expect(toCreateArgs(createInitialState({ name: "Explicit", folder: "/a/b" })).name).toBe("Explicit");
    expect(toCreateArgs(createInitialState({ name: "", folder: "/a/b/proj" })).name).toBe("proj");
    expect(toCreateArgs(createInitialState({ name: "", folder: "" })).name).toBe("workspace");
  });

  it("repo is the trimmed folder", () => {
    // set a name too, so this case isolates repo-trimming from basename-of-untrimmed
    const args = toCreateArgs(createInitialState({ name: "Keep", folder: "  /padded/path  " }));
    expect(args.repo).toBe("/padded/path");
  });
});

describe("gridShape", () => {
  it("maps count -> {rows, cols} via ceil(sqrt) cols, ceil(count/cols) rows", () => {
    expect(gridShape(1)).toEqual({ rows: 1, cols: 1 });
    expect(gridShape(2)).toEqual({ rows: 1, cols: 2 }); // cols >= rows
    expect(gridShape(4)).toEqual({ rows: 2, cols: 2 }); // 2x2
    expect(gridShape(6)).toEqual({ rows: 2, cols: 3 });
    expect(gridShape(9)).toEqual({ rows: 3, cols: 3 }); // 3x3
    expect(gridShape(10)).toEqual({ rows: 3, cols: 4 });
    expect(gridShape(12)).toEqual({ rows: 3, cols: 4 });
  });
});

describe("per-pane roles (17-01)", () => {
  it("createInitialState seeds roles[] to 'none', length === count", () => {
    const s = createInitialState({ count: 3 });
    expect(s.roles).toEqual([DEFAULT_ROLE, DEFAULT_ROLE, DEFAULT_ROLE]);
    expect(s.roles.length).toBe(s.count);
    expect(DEFAULT_ROLE).toBe("none");
    expect(KNOWN_ROLES).toContain("scout");
  });

  it("setPaneRole sets one pane, leaves the rest, never mutates input", () => {
    const s0 = createInitialState({ count: 3 });
    const before = snap(s0);
    const s1 = setPaneRole(s0, 1, "builder");
    expect(s1.roles).toEqual(["none", "builder", "none"]);
    expect(s0).toEqual(before); // input untouched (fresh array)
    expect(setPaneRole(s1, 9, "scout")).toBe(s1); // out of range = no-op (same ref)
    expect(setPaneRole(s1, -1, "scout")).toBe(s1);
  });

  it("setPaneModel sets one pane's model, fits + never mutates; toCreateArgs carries it", () => {
    const s0 = createInitialState({ count: 3 });
    const before = snap(s0);
    let s1 = setPaneModel(s0, 1, "claude-opus-4-8");
    expect(s1.models).toEqual(["", "claude-opus-4-8", ""]);
    expect(s0).toEqual(before); // input untouched
    expect(setPaneModel(s1, 9, "x")).toBe(s1); // out of range = no-op (same ref)
    s1 = setCount(s1, 4); // grow pads "" — the pick survives
    expect(s1.models).toEqual(["", "claude-opus-4-8", "", ""]);
    expect(toCreateArgs(s1).models).toEqual(["", "claude-opus-4-8", "", ""]);
    s1 = setCount(s1, 1); // shrink truncates
    expect(s1.models).toEqual([""]);
  });

  it("setCount fits roles: pads new panes with 'none', truncates extras", () => {
    let s = createInitialState({ count: 2 });
    s = setPaneRole(s, 0, "coordinator");
    s = setPaneRole(s, 1, "scout");
    s = setCount(s, 4); // grow: existing kept, new panes 'none'
    expect(s.roles).toEqual(["coordinator", "scout", "none", "none"]);
    expect(s.roles.length).toBe(4);
    s = setCount(s, 1); // shrink: truncate to count
    expect(s.roles).toEqual(["coordinator"]);
  });

  it("toCreateArgs carries roles parallel to harnesses", () => {
    let s = createInitialState({ count: 2, defaultHarness: "claude" });
    s = setPaneRole(s, 0, "coordinator");
    s = setPaneRole(s, 1, "builder");
    const args = toCreateArgs(s);
    expect(args.roles).toEqual(["coordinator", "builder"]);
    expect(args.harnesses.length).toBe(args.roles.length);
    expect(args.count).toBe(2);
  });

  it("applyPreset resets roles to 'none' for the preset count (presets are harness-only)", () => {
    let s = createInitialState({ count: 1 });
    s = setPaneRole(s, 0, "reviewer");
    s = applyPreset(s, fakePreset({ count: 2, harnesses: ["claude", "cursor"] }));
    expect(s.roles).toEqual(["none", "none"]);
    expect(s.roles.length).toBe(s.count);
  });

  it("openWithoutAI clears roles to 'none' (bash panes are roleless)", () => {
    let s = createInitialState({ count: 2 });
    s = setPaneRole(s, 0, "scout");
    s = openWithoutAI(s);
    expect(s.harnesses).toEqual(["bash", "bash"]);
    expect(s.roles).toEqual(["none", "none"]);
  });
});

describe("overflowCount — cap-aware queue hint (D33)", () => {
  it("returns 0 when the cap is unknown (browser / pre-poll)", () => {
    expect(overflowCount(createInitialState({ count: 9 }))).toBe(0); // no cap opt → cap null
  });
  it("returns 0 when the panes fit the free slots", () => {
    expect(overflowCount(createInitialState({ count: 2, cap: 4, working: 0 }))).toBe(0);
  });
  it("overflow = count - (cap - working): 4 panes, cap 4, 2 working → 2 free → 2 overflow", () => {
    expect(overflowCount(createInitialState({ count: 4, cap: 4, working: 2 }))).toBe(2);
  });
  it("a full cap → every pane overflows", () => {
    expect(overflowCount(createInitialState({ count: 3, cap: 2, working: 2 }))).toBe(3);
  });
});
