//! `adapter` — headless ranked-table CLI over the cross-harness state adapter
//! (Plan 01-01, Task 4). The working precursor to the Phase-02 Queue UI.
//!
//! Usage:
//!   adapter watch [STATE_DIR]                      # live: redraw on change
//!   adapter once  [STATE_DIR]                      # print the table once
//!   adapter inject <cursor|claude> <id> <repo-dir> # inject hooks (AGENT_TEAMS_HOOKS_DIR)
//!
//! STATE_DIR defaults to $AGENT_TEAMS_STATE_DIR, else
//! ~/Library/Application Support/harness-ready/agent-teams.

use state_adapter::inject::{inject, InjectConfig, InjectHarness};
use state_adapter::watch::{current_states, discover};
use state_adapter::{Harness, State, WaitingReason};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("watch");

    match mode {
        "once" => {
            let dir = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(default_state_dir);
            print!("{}", render(&dir));
        }
        "watch" => {
            let dir = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(default_state_dir);
            watch_loop(&dir);
        }
        "inject" => cmd_inject(&args),
        other => {
            eprintln!("unknown mode '{other}'. usage: adapter [watch|once|inject] ...");
            std::process::exit(2);
        }
    }
}

/// `adapter inject <cursor|claude> <workspace-id> <repo-dir>` — render the
/// harness hook config into a workspace (dogfoods `state_adapter::inject`).
fn cmd_inject(args: &[String]) {
    let usage =
        "usage: adapter inject <cursor|claude> <workspace-id> <repo-dir>  (set AGENT_TEAMS_HOOKS_DIR)";
    let harness = match args.get(1).map(String::as_str) {
        Some("cursor") => InjectHarness::Cursor,
        Some("claude") => InjectHarness::Claude,
        _ => fail(usage),
    };
    let wsid = args.get(2).cloned().unwrap_or_else(|| fail(usage));
    let repo = args
        .get(3)
        .map(PathBuf::from)
        .unwrap_or_else(|| fail(usage));
    let hooks = std::env::var("AGENT_TEAMS_HOOKS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| fail("AGENT_TEAMS_HOOKS_DIR unset (point it at core/hooks/)"));
    let cfg = InjectConfig {
        workspace_id: wsid.clone(),
        repo_dir: repo,
        writer_path: hooks.join("state-writer.sh"),
        templates_dir: hooks,
        // state_root is only consulted by inject_mcp_config (16-01); this CLI only
        // dogfoods hook `inject`, so derive it best-effort from the env (unused here).
        state_root: std::env::var("AGENT_TEAMS_STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_default(),
        // Only rendered by inject_mcp_config; unused by this hook-only dogfood CLI.
        pane_id: wsid.clone(),
        repo_key: wsid.clone(),
        task_scope: wsid,
    };
    // "" → no env merge: this std-only dogfood CLI can't parse JSON (serde_json is a
    // dev-dep). The real env-merge path is the supervisor (Bedrock etc.), not this tool.
    match inject(&cfg, harness, "") {
        Ok(p) => println!("injected {} → {}", harness_str_inject(harness), p.display()),
        Err(e) => {
            eprintln!("inject failed: {e}");
            std::process::exit(1);
        }
    }
}

fn fail<T>(msg: &str) -> T {
    eprintln!("{msg}");
    std::process::exit(2);
}

fn harness_str_inject(h: InjectHarness) -> &'static str {
    match h {
        InjectHarness::Cursor => "cursor",
        InjectHarness::Claude => "claude",
    }
}

fn default_state_dir() -> PathBuf {
    if let Ok(d) = std::env::var("AGENT_TEAMS_STATE_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Application Support/harness-ready/agent-teams")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Poll the state dir and redraw only when the ranked snapshot changes.
fn watch_loop(dir: &Path) {
    let mut last = String::new();
    loop {
        let frame = render(dir);
        if frame != last {
            print!("\x1b[2J\x1b[H{frame}"); // clear + home, then table
            last = frame;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn render(dir: &Path) -> String {
    let rows = current_states(&discover(dir));
    let now = now_ms();
    let mut out = String::new();
    out.push_str(&format!(
        "Agent Teams — who needs you  ({})\n",
        dir.display()
    ));
    out.push_str("  WORKSPACE            HARNESS  STATE     REASON     NEEDS YOU  WAIT\n");
    if rows.is_empty() {
        out.push_str("  (no workspaces — nothing waiting)\n");
        return out;
    }
    for (id, harness, st) in rows {
        let mark = if st.needs_human { "►" } else { " " };
        out.push_str(&format!(
            "{mark} {:<20} {:<8} {:<9} {:<10} {:<9}  {}\n",
            truncate(&id, 20),
            harness_str(harness),
            state_str(st.state),
            reason_str(st.waiting_reason),
            if st.needs_human { "YES" } else { "-" },
            wait_str(now, st.since),
        ));
    }
    out
}

fn harness_str(h: Harness) -> &'static str {
    match h {
        Harness::Claude => "claude",
        Harness::Cursor => "cursor",
        Harness::Codex => "codex",
        Harness::CommandCode => "commandcode",
        Harness::OpenCode => "opencode",
        Harness::Cline => "cline",
        Harness::Grok => "grok",
    }
}

fn state_str(s: State) -> &'static str {
    match s {
        State::Idle => "idle",
        State::Working => "working",
        State::Waiting => "waiting",
        State::Done => "done",
        State::Error => "error",
    }
}

fn reason_str(r: Option<WaitingReason>) -> &'static str {
    match r {
        Some(WaitingReason::Approval) => "approval",
        Some(WaitingReason::Question) => "question",
        Some(WaitingReason::TurnEnd) => "turn_end",
        Some(WaitingReason::RateLimit) => "rate_limit",
        None => "-",
    }
}

fn wait_str(now: u64, since: u64) -> String {
    if since == 0 || now < since {
        return "-".into();
    }
    let secs = (now - since) / 1000;
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m{}s", secs / 60, secs % 60)
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let cut: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}
