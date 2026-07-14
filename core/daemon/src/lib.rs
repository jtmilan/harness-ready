//! Agent Teams — detached daemon **skeleton** (Phase 08 Sub-build 1).
//!
//! This crate is the LAUNCH-AGNOSTIC skeleton of the detached daemon that will
//! eventually own the PTY-backed agent sessions and survive the GUI app's
//! lifecycle (`.paul/phases/08-daemon/08-01-PLAN.md`). It deliberately contains
//! only **decision logic** and **trait / placeholder surfaces** — it owns NO PTY
//! master fd and NO socket server, and `launch` is a *gated stub*. Those land in
//! later sub-builds (Sub-build 2 = the `sups` PTY migration; the socket server +
//! streaming after).
//!
//! ## The one invariant this skeleton already enforces
//!
//! **Idle = ZERO LIVE PANES, NOT "no GUI attached"** (AC-4 / hard-problem #2).
//! Quitting the GUI while an agent still runs must NOT shut the daemon down — only
//! a sustained *zero-live-pane* window past the grace does. [`lifecycle`] is that
//! decision, as a pure function with no internal clock.
//!
//! ## SSOT
//!
//! The durable IPC/registry path policy lives ONCE, in `agent-teams-core`
//! (`core/mcp`): [`agent_teams_core::socket_path`] /
//! [`agent_teams_core::registry_path`] / [`agent_teams_core::LiveRegistry`]. This
//! crate REUSES them by path-dep and never redefines them. The ONLY new path code
//! is the daemon's own run-state file in [`runstate`], which *mirrors*
//! `registry_path` (sibling-of-`state_root`) so the two can never drift.
//!
//! ## Module map
//!
//! * [`lifecycle`]  — pure idle-shutdown decision (AC-4 core).
//! * [`idle_tick`]  — real-time idle-tick driver: owns the clock, calls `lifecycle`
//!                    on a configurable interval, fires `on_shutdown` when the grace
//!                    elapses (08-T8).
//! * [`runstate`]   — the daemon's durable run-state file (sibling of `state_root`).
//! * [`launch`]     — the launch-mechanism trait + A1 launchd impl (real
//!                    `launch_activate_socket` FFI, D45 / 08-T2) + A2 dev self-bind.
//! * [`sups`]       — the daemon-owned live-pane map ([`sups::DaemonSups`]) whose
//!                    `count_live()` feeds [`lifecycle`] (Sub-build 2 / 08-T4).
//! * [`handlers`]   — pure write/read/resize/list request handlers over `DaemonSups`
//!                    (Sub-build 2 machinery; the socket server that calls them is
//!                    Sub-build 3).
//! * [`server`]     — the Sub-build 3 (slice 2) accept-loop socket SERVER: per-connection
//!                    threads (MF-A), euid + fresh-`allow_mutations` gates (MF-C/MF-D),
//!                    op routing. Streaming (Attach/Detach) lands in slice 3.
//! * [`registry_writer`] — the daemon's `agent-teams-live.json` writer (its own pid +
//!                    real child pids); built now, driven in Sub-build 3.
//! * [`reattach`]   — the pure re-attach-vs-cold-resume partition (AC-2/AC-6).
//! * [`install`]    — LaunchAgent plist generation for A1 socket-activation (D45).

// The crate-doc module map above uses deliberate column alignment.
#![allow(clippy::doc_overindented_list_items)]

pub mod frames;
pub mod fsutil;
pub mod handlers;
pub mod idle_tick;
pub mod install;
pub mod launch;
pub mod lifecycle;
pub mod reattach;
pub mod registry_writer;
pub mod runstate;
pub mod server;
pub mod sups;

// Q4 daemon-spawns-on-behalf (approach B). Compiled out of the DEFAULT build (byte-inert)
// — present only under the `daemon-spawn` feature (live production wiring) or `cfg(test)`
// (so `cargo test` exercises the gated handler/lifecycle logic without the feature flag).
#[cfg(any(test, feature = "daemon-spawn"))]
pub mod audit;
#[cfg(any(test, feature = "daemon-spawn"))]
pub mod spawn;
