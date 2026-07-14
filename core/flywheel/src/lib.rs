//! `agent_teams_core` — pure, headless core for the `ade` CLI.
//!
//! Phase 0 (extraction spike): lift the *already-pure* functions out of the
//! agent-teams Tauri app (`app/src-tauri/src/lib.rs`) into a GUI-free crate so a
//! thin `ade` binary, the Tauri app, and the MCP sidecar can all consume one core.
//!
//! **Invariant (to be CI-asserted): this crate must NOT reference Tauri
//! `AppHandle`/`AppState`.** A `grep AppState|AppHandle` over
//! `core/flywheel/src` must return zero hits — that is the de-risk gate for
//! the whole port.
//!
//! Extraction status (source line refs are into agent-teams `app/src-tauri/src/lib.rs`):
//! - [x] `model::resolve_headless_model`  — extracted + 2 tests (~L3821). Proof of the pure boundary.
//! - [x] `orchestrate::orchestrate_sync`  — extracted (with `PaneCtx`/`Dispatch`/`Orchestration` + `build_orchestration_prompt`/`parse_orchestration`).
//! - [x] `synthesize::synthesize_core`    — extracted (the ONE fan-in core; includes `run_authoritative_tests` + `build_synthesis_prompt`).
//! - [x] `flywheel::flywheel_push_and_pr` — extracted (push integ branch + `gh pr create`).
//! - [x] `runctx::RunContext`             — extracted (owns run state; RAII `Drop` cleanup).

pub mod apply;
pub mod flywheel;
pub(crate) mod gitutil;
pub(crate) mod gitwt;
pub mod model;
pub mod orchestrate;
pub mod runctx;
pub mod runlog;
pub mod synthesize;
pub mod worker;
