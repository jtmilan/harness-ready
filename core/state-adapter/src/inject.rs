//! Per-workspace hook injection (Plan 01-01, Task 3).
//!
//! Renders a harness hook config from a template into the workspace, pointing
//! every lifecycle hook at `state-writer.sh`. Event data lands under
//! `$AGENT_TEAMS_STATE_DIR` (app-support) — never the repo (D6) — and the
//! generated config is excluded from the repo's tracked tree so `git status`
//! stays clean (AC-2).
//!
//! std-only (no external deps).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
pub enum InjectHarness {
    Claude,
    Cursor,
}

#[derive(Debug, Clone)]
pub struct InjectConfig {
    pub workspace_id: String,
    /// the target repo / worktree to inject into
    pub repo_dir: PathBuf,
    /// absolute path to the injected `state-writer.sh`
    pub writer_path: PathBuf,
    /// `core/hooks/` — where the `*.tmpl.json` live. Also where the per-pane
    /// claude MCP config is staged (it is OUTSIDE the worktree by construction:
    /// in the packaged app this is a sibling of `state_root`; in dev it is the
    /// source tree's `core/hooks` — neither is inside a user worktree).
    pub templates_dir: PathBuf,
    /// the app's `AGENT_TEAMS_STATE_DIR` — written EXPLICITLY into the injected
    /// MCP config's `env` block so the sidecar resolves the same live state the
    /// app does, without relying on env inheriting two hops down (D56, R6).
    pub state_root: PathBuf,
    /// the pane's own id — written into the MCP `env` block as `AGENT_TEAMS_PANE_ID`
    /// (enablement slice, C2 provenance). Same proven channel as `state_root`: the
    /// sidecar reads it from its own process env, not a fragile two-hop inherit.
    pub pane_id: String,
    /// the team-shared memory repo-key — written as `AGENT_TEAMS_MEMORY_REPO_KEY`
    /// (C7) so a workspace's panes share ONE memory store instead of fragmenting
    /// per worktree cwd.
    pub repo_key: String,
    /// the per-pane task transition scope — written as `AGENT_TEAMS_TASK_SCOPE` (4c)
    /// so a pane may advance only the tasks it authored.
    pub task_scope: String,
}

impl InjectHarness {
    /// (template filename, path relative to repo_dir where the config is written)
    fn template_and_target(self) -> (&'static str, &'static str) {
        match self {
            // cursor reads project-local .cursor/hooks.json (proven by the spike)
            InjectHarness::Cursor => ("cursor-hooks.tmpl.json", ".cursor/hooks.json"),
            // claude reads .claude/settings.local.json (the gitignored local override)
            InjectHarness::Claude => ("claude-hooks.tmpl.json", ".claude/settings.local.json"),
        }
    }
}

/// True iff `dir` is inside an agent-teams-created DISPOSABLE worktree (some path
/// component is `.agent-teams-worktrees` — the sanctioned per-id location every
/// worktree-creating path in the app/daemon/flywheel uses). When a pane's cwd is NOT
/// one — the no-worktree fallback roots the pane at the user's REAL folder — the inject
/// paths below must not silently clobber the user's own config files.
fn is_agent_teams_worktree(dir: &Path) -> bool {
    let canon = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    canon
        .components()
        .any(|c| c.as_os_str() == ".agent-teams-worktrees")
}

/// Guarded config write: inside a disposable agent-teams worktree this is a plain
/// overwrite (the tree is ephemeral — today's behavior). OUTSIDE one (the pane is
/// rooted at the user's REAL folder), a pre-existing file with different content is
/// FIRST backed up to `<name>.agent-teams-backup` (and the overwrite is logged), so
/// injection never destroys the user's own `.claude/settings.local.json` /
/// `.cursor/hooks.json` / `.cursor/mcp.json` / `.mcp.json`.
fn write_config_guarded(repo_dir: &Path, out: &Path, rendered: &str) -> io::Result<()> {
    if !is_agent_teams_worktree(repo_dir) && out.exists() {
        let existing = fs::read_to_string(out).unwrap_or_default();
        if existing != rendered {
            let mut backup_os = out.as_os_str().to_owned();
            backup_os.push(".agent-teams-backup");
            let backup = PathBuf::from(backup_os);
            fs::copy(out, &backup)?;
            eprintln!(
                "[agent-teams] {} is a REAL user file (pane cwd is not an agent-teams worktree) — \
                 backed it up to {} before overwriting",
                out.display(),
                backup.display()
            );
        }
    }
    fs::write(out, rendered)
}

/// Inject the harness hooks into `cfg.repo_dir`. Returns the path written.
///
/// `env_block` is a pre-rendered JSON fragment spliced into the claude template's
/// `{{ENV_BLOCK}}` slot — the original Phase-02 "MERGE rather than clobber" gap. A
/// fresh worktree never has its own `.claude/settings.local.json` (`.claude/` is
/// gitignored, so it is never in the checkout), so any per-repo `env` the operator
/// keeps in the SOURCE repo — e.g. the Bedrock switch (`CLAUDE_CODE_USE_BEDROCK` /
/// `AWS_PROFILE` / `ANTHROPIC_DEFAULT_*`) — would otherwise be dropped and the pane
/// would silently fall back to the account default. The CALLER computes the fragment
/// (it needs a JSON parse; this crate stays std-only) and passes `""` to keep the
/// prior clobber behavior. The cursor template has no `{{ENV_BLOCK}}` token, so a
/// non-empty `env_block` is a harmless no-op there.
pub fn inject(cfg: &InjectConfig, harness: InjectHarness, env_block: &str) -> io::Result<PathBuf> {
    let (tmpl_name, target_rel) = harness.template_and_target();
    let tmpl = fs::read_to_string(cfg.templates_dir.join(tmpl_name))?;
    let rendered = tmpl
        .replace("{{WSID}}", &cfg.workspace_id)
        .replace("{{WRITER}}", &cfg.writer_path.to_string_lossy())
        .replace("{{ENV_BLOCK}}", env_block);

    let out = cfg.repo_dir.join(target_rel);
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    write_config_guarded(&cfg.repo_dir, &out, &rendered)?;

    // keep `git status` clean — the generated config is tooling, not project source
    exclude_from_git(&cfg.repo_dir, target_rel)?;
    Ok(out)
}

/// Inject the read-only `agent-teams-mcp` stdio sidecar into a pane (Plan 16-01,
/// item 1 — D56). SEPARATE from [`inject`] (which writes hooks) so item 2
/// (mutation/memory wiring) and 17-01 (role rule) rebase on a small, distinct
/// touch point — do NOT fold this into `inject`.
///
/// Renders the per-harness `*-mcp.tmpl.json` (filling `{{SIDECAR}}` = the
/// absolute bundled sidecar path and `{{STATE_DIR}}` = `cfg.state_root`) and:
///
/// - **Claude** → writes the config to a STAGED sibling OUTSIDE the worktree
///   (`<templates_dir>/<wsid>-claude-mcp.json`) and returns `Some(path)`. The
///   caller passes it via `--mcp-config <abs>` (additive, NOT `--strict`). No
///   `.git/info/exclude` entry — the file is not in the worktree, so `git status`
///   stays clean for free. Written per-pane so item 2's per-pane `env` value
///   needs no shared-file refactor.
/// - **Cursor** → writes `<repo_dir>/.cursor/mcp.json` IN the worktree (cursor
///   discovers it by path), excludes it via `.git/info/exclude`, returns `None`.
/// - **Bash** never reaches here (the supervisor skips injection for Bash).
///
/// `sidecar_bin` is the resolved absolute path to the bundled binary. A failed
/// resolution is the caller's concern (degrade to "no MCP in the pane"); this fn
/// is only called with a real path.
/// The optional **BridgeAgent** MCP server binary, injected into every (non-worker) pane ALONGSIDE
/// agent-teams so any harness can reach it for cross-agent communication. Auto-detected —
/// portable and default-off: `$AGENT_TEAMS_BRIDGEAGENT_BIN` if set and a real file, else the
/// canonical BRIDGE_HOME path `$HOME/.bridgeagent/bin/bridge`. `None` when BridgeAgent isn't
/// installed → only agent-teams is injected (nothing forced on machines without it).
fn bridgeagent_bin() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("AGENT_TEAMS_BRIDGEAGENT_BIN") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let home = std::env::var("HOME").ok()?;
    // Candidates in priority order: BridgeAgent's documented home-bin, then the uv-tool PATH symlink
    // (where `uv tool install bridgeagent` actually lands the `bridge` entrypoint). `is_file()`
    // follows symlinks → a uv-tool symlink to the launcher resolves true. First hit wins.
    [".bridgeagent/bin/bridge", ".local/bin/bridge"]
        .into_iter()
        .map(|rel| PathBuf::from(&home).join(rel))
        .find(|p| p.is_file())
}

/// The `{{BRIDGEAGENT}}` template fragment — a leading-comma JSON entry adding the `bridgeagent`
/// server right after agent-teams, or `""` when BridgeAgent isn't installed (default-off). Uses the
/// flat claude/cursor/commandcode shape (`{command,args}`). PURE → unit-tested. Kept as string
/// templating (not serde) so the SHIPPED lib stays std-only — the templates are placeholder-
/// rendered, never JSON-parsed at runtime. `bridge mcp serve` is the stdio command
/// (BridgeAgent's own client snippet).
fn bridgeagent_fragment(bin: Option<&Path>) -> String {
    let Some(bin) = bin else { return String::new() };
    // Minimal JSON-string escaping of the path value (macOS paths rarely need it — be safe anyway).
    let cmd = bin
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!(",\n      \"bridgeagent\": {{ \"command\": \"{cmd}\", \"args\": [\"mcp\", \"serve\"] }}")
}

pub fn inject_mcp_config(
    cfg: &InjectConfig,
    harness: InjectHarness,
    sidecar_bin: &Path,
) -> io::Result<Option<PathBuf>> {
    let (tmpl_name, write_target) = match harness {
        // claude → staged sibling, passed by absolute path via --mcp-config
        InjectHarness::Claude => ("claude-mcp.tmpl.json", McpTarget::StagedSibling),
        // cursor → in-worktree .cursor/mcp.json (discovered by path) + git-exclude
        InjectHarness::Cursor => (
            "cursor-mcp.tmpl.json",
            McpTarget::InWorktree(".cursor/mcp.json"),
        ),
    };

    let tmpl = fs::read_to_string(cfg.templates_dir.join(tmpl_name))?;
    let rendered = tmpl
        .replace("{{SIDECAR}}", &sidecar_bin.to_string_lossy())
        .replace("{{STATE_DIR}}", &cfg.state_root.to_string_lossy())
        .replace("{{PANE_ID}}", &cfg.pane_id)
        .replace("{{REPO_KEY}}", &cfg.repo_key)
        .replace("{{TASK_SCOPE}}", &cfg.task_scope)
        // Cross-agent comms: add the BridgeAgent server alongside agent-teams (flat schema; "" if
        // BridgeAgent isn't installed).
        .replace(
            "{{BRIDGEAGENT}}",
            &bridgeagent_fragment(bridgeagent_bin().as_deref()),
        );

    match write_target {
        McpTarget::StagedSibling => {
            // <templates_dir>/<wsid>-claude-mcp.json — outside the worktree.
            let out = cfg
                .templates_dir
                .join(format!("{}-claude-mcp.json", cfg.workspace_id));
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&out, rendered)?;
            Ok(Some(out))
        }
        McpTarget::InWorktree(rel) => {
            let out = cfg.repo_dir.join(rel);
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            write_config_guarded(&cfg.repo_dir, &out, &rendered)?;
            // keep `git status` clean — generated tooling, not project source
            exclude_from_git(&cfg.repo_dir, rel)?;
            Ok(None)
        }
    }
}

/// Inject a per-role persona into a CURSOR pane as a project-local Cursor rule
/// (Plan 17-01). cursor-agent has NO `--append-system-prompt` flag, so a written
/// `.cursor/rules/*.mdc` rule is its only persistent steering — the EXACT mechanism
/// [`inject`] / [`inject_mcp_config`] already use for `.cursor/hooks.json` /
/// `.cursor/mcp.json` (project-local file + `.git/info/exclude`).
///
/// Renders `cursor-role.tmpl.md` (filling `{{PERSONA}}` = the role's system-prompt
/// body) → writes `<repo_dir>/.cursor/rules/agent-role.mdc` → excludes it from git so
/// `git status` stays clean. Returns the path written. Additive sibling to [`inject`]
/// and [`inject_mcp_config`] — do NOT fold this into either.
///
/// Claude does NOT use this (it gets `--append-system-prompt` on the CLI); bash never
/// reaches here. `templates_dir` is `core/hooks/` (where the `*.tmpl.md` live).
pub fn inject_cursor_role(
    repo_dir: &Path,
    templates_dir: &Path,
    persona: &str,
) -> io::Result<PathBuf> {
    const REL: &str = ".cursor/rules/agent-role.mdc";
    let tmpl = fs::read_to_string(templates_dir.join("cursor-role.tmpl.md"))?;
    let rendered = tmpl.replace("{{PERSONA}}", persona);

    let out = repo_dir.join(REL);
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    write_config_guarded(repo_dir, &out, &rendered)?;

    // keep `git status` clean — the rule is generated tooling, not project source
    exclude_from_git(repo_dir, REL)?;
    Ok(out)
}

/// Inject the agent-teams stdio MCP sidecar into a COMMANDCODE pane as a project-local
/// `.mcp.json` at the worktree root — Command Code auto-discovers it (verified via
/// `commandcode mcp list`). commandcode is STATE-BLIND (no hooks) but a first-class MCP
/// CLIENT, so it reads the queue + the gated memory/task tools like cursor
/// (D56/D60). Mirrors [`inject_mcp_config`]'s cursor branch — SAME schema, the
/// `/bin/sh -c "exec '<path>'"` wrapper (commandcode, like cursor, would split a spaced
/// bundle path), and the SAME provenance env block (STATE_DIR/PANE_ID/REPO_KEY/
/// TASK_SCOPE) — but writes `<worktree>/.mcp.json` (commandcode's project scope) and has
/// a SEPARATE entry point (commandcode has no `InjectHarness`, being `inject: None`).
///
/// Like cursor's `.cursor/mcp.json` it OVERWRITES + git-excludes: the worktree is
/// disposable (`.agent-teams-worktrees/<id>`), so a repo's own committed `.mcp.json` is
/// shadowed only inside this ephemeral pane, never the real checkout. Additive sibling
/// to [`inject_mcp_config`] / [`inject_cursor_role`] — do NOT fold in. std-only. Also
/// used for OpenCode (same `.mcp.json` project-root discovery mechanism).
pub fn inject_commandcode_mcp(
    repo_dir: &Path,
    templates_dir: &Path,
    sidecar_bin: &Path,
    state_root: &Path,
    pane_id: &str,
    repo_key: &str,
) -> io::Result<PathBuf> {
    const REL: &str = ".mcp.json";
    let tmpl = fs::read_to_string(templates_dir.join("commandcode-mcp.tmpl.json"))?;
    let rendered = tmpl
        .replace("{{SIDECAR}}", &sidecar_bin.to_string_lossy())
        .replace("{{STATE_DIR}}", &state_root.to_string_lossy())
        .replace("{{PANE_ID}}", pane_id)
        .replace("{{REPO_KEY}}", repo_key)
        .replace("{{TASK_SCOPE}}", pane_id) // 4c: own-task transition scope = pane id
        // Cross-agent comms: add BridgeAgent alongside agent-teams (flat schema; "" if not installed).
        .replace(
            "{{BRIDGEAGENT}}",
            &bridgeagent_fragment(bridgeagent_bin().as_deref()),
        );

    let out = repo_dir.join(REL);
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    write_config_guarded(repo_dir, &out, &rendered)?;
    // keep `git status` clean — generated tooling, not project source (idempotent)
    exclude_from_git(repo_dir, REL)?;
    Ok(out)
}

/// Inject the Agent Teams MCP sidecar into an OpenCode pane's project-scoped config.
/// OpenCode reads `opencode.json` at the project root (NOT `.mcp.json` — that format is
/// Claude/Cursor-specific and OpenCode silently ignores it, GitHub issue #27809 "Closed
/// as not planned"). The MCP key in `opencode.json` is `"mcp"` (not `"mcpServers"`),
/// with `type: "local"`, `command` as a string array, and `environment` (not `env`).
/// Same provenance env block (STATE_DIR/PANE_ID/REPO_KEY/TASK_SCOPE).
///
/// OVERWRITES + git-excludes: the worktree is disposable, so a repo's own committed
/// `opencode.json` is shadowed only inside this ephemeral pane. Additive sibling to
/// [`inject_commandcode_mcp`] / [`inject_mcp_config`] — do NOT fold in. std-only.
/// OpenCode merges project-level config with global (`~/.config/opencode/opencode.json`)
/// so the pane's user-configured models/providers are preserved; only `mcp` is added.
pub fn inject_opencode_mcp(
    repo_dir: &Path,
    templates_dir: &Path,
    sidecar_bin: &Path,
    state_root: &Path,
    pane_id: &str,
    repo_key: &str,
) -> io::Result<PathBuf> {
    const REL: &str = "opencode.json";
    let tmpl = fs::read_to_string(templates_dir.join("opencode-mcp.tmpl.json"))?;
    let rendered = tmpl
        .replace("{{SIDECAR}}", &sidecar_bin.to_string_lossy())
        .replace("{{STATE_DIR}}", &state_root.to_string_lossy())
        .replace("{{PANE_ID}}", pane_id)
        .replace("{{REPO_KEY}}", repo_key)
        .replace("{{TASK_SCOPE}}", pane_id);

    let out = repo_dir.join(REL);
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    write_config_guarded(repo_dir, &out, &rendered)?;
    // keep `git status` clean — generated tooling, not project source (idempotent)
    exclude_from_git(repo_dir, REL)?;
    Ok(out)
}

/// Inject the Agent Teams MCP sidecar into a GROK pane's project-scoped config.
/// Grok reads `.grok/config.toml` at the project root (discovered by path, like
/// cursor's `.cursor/mcp.json`). The template is TOML (not JSON) — grok's native
/// config format. Same provenance env block (STATE_DIR/PANE_ID/REPO_KEY/TASK_SCOPE).
///
/// OVERWRITES + git-excludes: the worktree is disposable, so a repo's own committed
/// `.grok/config.toml` is shadowed only inside this ephemeral pane. Additive sibling
/// to [`inject_commandcode_mcp`] / [`inject_mcp_config`] — do NOT fold in. std-only.
pub fn inject_grok_mcp(
    repo_dir: &Path,
    templates_dir: &Path,
    sidecar_bin: &Path,
    state_root: &Path,
    pane_id: &str,
    repo_key: &str,
) -> io::Result<PathBuf> {
    const REL: &str = ".grok/config.toml";
    let tmpl = fs::read_to_string(templates_dir.join("grok-mcp.tmpl.toml"))?;
    let rendered = tmpl
        .replace("{{SIDECAR}}", &sidecar_bin.to_string_lossy())
        .replace("{{STATE_DIR}}", &state_root.to_string_lossy())
        .replace("{{PANE_ID}}", pane_id)
        .replace("{{REPO_KEY}}", repo_key)
        .replace("{{TASK_SCOPE}}", pane_id);

    let out = repo_dir.join(REL);
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    write_config_guarded(repo_dir, &out, &rendered)?;
    // keep `git status` clean — generated tooling, not project source (idempotent)
    exclude_from_git(repo_dir, REL)?;
    Ok(out)
}

/// Where an injected MCP config lands (claude vs cursor mechanism).
enum McpTarget {
    /// Claude: a staged file OUTSIDE the worktree, passed by absolute path.
    StagedSibling,
    /// Cursor: a path relative to the worktree, discovered by the harness.
    InWorktree(&'static str),
}

/// Append `rel` to the repo's `.git/info/exclude` (idempotent). No-op if the
/// directory isn't a git repo.
/// Resolve the `info/exclude` file git ACTUALLY reads for `repo_dir` — std-only,
/// matching `git rev-parse --git-path info/exclude`:
/// - normal repo (`<repo>/.git` is a DIR) → `<repo>/.git/info/exclude`.
/// - linked WORKTREE (`<repo>/.git` is a FILE `gitdir: <gitdir>`) → the SHARED exclude
///   in the common dir (`<gitdir>/<commondir>/info/exclude`, e.g. `../..` → `<main>/.git`).
///   `info/exclude` lives in the common dir, NOT the per-worktree gitdir.
///
/// Returns `None` when `repo_dir` is not a git repo (nothing to exclude).
fn git_exclude_path(repo_dir: &Path) -> Option<PathBuf> {
    let dotgit = repo_dir.join(".git");
    let meta = fs::symlink_metadata(&dotgit).ok()?;
    if meta.is_dir() {
        return Some(dotgit.join("info").join("exclude"));
    }
    // linked worktree: `.git` is a file → `gitdir: <abs per-worktree gitdir>`
    let content = fs::read_to_string(&dotgit).ok()?;
    let gitdir = content.lines().next()?.strip_prefix("gitdir:")?.trim();
    let gitdir = PathBuf::from(gitdir);
    // `<gitdir>/commondir` points to the common dir (relative to gitdir, or absolute);
    // absent ⇒ the gitdir IS the common dir.
    let common = match fs::read_to_string(gitdir.join("commondir")) {
        Ok(s) => {
            let rel = Path::new(s.trim()).to_path_buf();
            if rel.is_absolute() {
                rel
            } else {
                gitdir.join(rel)
            }
        }
        Err(_) => gitdir.clone(),
    };
    Some(common.join("info").join("exclude"))
}

/// Append `rel` to the repo's real `info/exclude` (idempotent, exact-LINE match). No-op
/// if `repo_dir` isn't a git repo OR the resolved `info/` dir is absent (a non-git
/// fixture). Worktree-aware (D70 follow-up): the prior `<repo>/.git/info` join silently
/// no-op'd in a linked worktree (`.git` is a FILE there) → injected `.mcp.json` /
/// `.cursor/mcp.json` were NOT actually excluded → a bridge `git add -A` (D42) could
/// commit a machine-specific config. `git_exclude_path` resolves the common-dir exclude.
fn exclude_from_git(repo_dir: &Path, rel: &str) -> io::Result<()> {
    let Some(exclude) = git_exclude_path(repo_dir) else {
        return Ok(()); // not a git repo — nothing to exclude
    };
    match exclude.parent() {
        Some(d) if d.exists() => {}
        _ => return Ok(()), // resolved info/ dir absent (non-git fixture) — nothing to do
    }
    let mut body = fs::read_to_string(&exclude).unwrap_or_default();
    if body.lines().any(|l| l == rel) {
        return Ok(()); // already excluded (exact line, not a substring)
    }
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(rel);
    body.push('\n');
    fs::write(&exclude, body)
}

#[cfg(test)]
mod mcp_tests {
    use super::*;

    // core/hooks (where the *.tmpl.json live) relative to this crate's manifest.
    fn templates_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hooks")
    }

    fn scratch(tag: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!("at-inject-mcp-{tag}-{nonce}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn cfg_for(tag: &str, repo_dir: PathBuf, staged: PathBuf, state_root: PathBuf) -> InjectConfig {
        InjectConfig {
            workspace_id: format!("ws-{tag}"),
            repo_dir,
            writer_path: PathBuf::from("/unused/state-writer.sh"),
            // route the claude staged file into an isolated dir (NOT the source
            // core/hooks) so the test never writes into the tree — but read the
            // real templates from `templates_dir()`.
            templates_dir: staged,
            state_root,
            // enablement slice (C2/C7/4c) — the env vars the sidecar reads.
            pane_id: format!("ws-{tag}-p0"),
            repo_key: format!("ws-{tag}"),
            task_scope: format!("ws-{tag}-p0"),
        }
    }

    // Item-17 guard: when the pane cwd is NOT an agent-teams worktree (the no-worktree
    // fallback roots the pane at the user's REAL folder), a pre-existing user config is
    // BACKED UP to <name>.agent-teams-backup before the overwrite.
    #[test]
    fn injection_outside_agent_teams_worktree_backs_up_real_user_file() {
        let root = scratch("guard-backup");
        let repo = root.join("real-user-folder"); // NOT under .agent-teams-worktrees
        let staged = root.join("staged-hooks");
        let state = root.join("app-state");
        fs::create_dir_all(repo.join(".cursor")).unwrap();
        fs::create_dir_all(&staged).unwrap();
        fs::copy(
            templates_dir().join("cursor-mcp.tmpl.json"),
            staged.join("cursor-mcp.tmpl.json"),
        )
        .unwrap();
        // the user's REAL pre-existing config
        let user_cfg = repo.join(".cursor/mcp.json");
        fs::write(&user_cfg, "{\"mcpServers\":{\"users-own\":{}}}").unwrap();

        let sidecar = PathBuf::from("/abs/path/to/agent-teams-mcp");
        let cfg = cfg_for("guard", repo.clone(), staged, state);
        inject_mcp_config(&cfg, InjectHarness::Cursor, &sidecar).expect("inject");

        // overwritten with the rendered config…
        let now = fs::read_to_string(&user_cfg).unwrap();
        assert!(now.contains("agent-teams"), "rendered config written");
        // …but the user's original was preserved first.
        let backup = repo.join(".cursor/mcp.json.agent-teams-backup");
        assert!(backup.exists(), "real user file backed up before overwrite");
        assert_eq!(
            fs::read_to_string(&backup).unwrap(),
            "{\"mcpServers\":{\"users-own\":{}}}",
            "backup carries the user's original content"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn injection_inside_agent_teams_worktree_overwrites_without_backup() {
        let root = scratch("guard-wt");
        // a DISPOSABLE agent-teams worktree path → plain overwrite (today's behavior)
        let repo = root.join(".agent-teams-worktrees").join("ws-1");
        let staged = root.join("staged-hooks");
        let state = root.join("app-state");
        fs::create_dir_all(repo.join(".cursor")).unwrap();
        fs::create_dir_all(&staged).unwrap();
        fs::copy(
            templates_dir().join("cursor-mcp.tmpl.json"),
            staged.join("cursor-mcp.tmpl.json"),
        )
        .unwrap();
        // pre-existing file from a prior spawn in the disposable tree
        fs::write(repo.join(".cursor/mcp.json"), "{\"stale\":true}").unwrap();

        let sidecar = PathBuf::from("/abs/path/to/agent-teams-mcp");
        let cfg = cfg_for("guard-wt", repo.clone(), staged, state);
        inject_mcp_config(&cfg, InjectHarness::Cursor, &sidecar).expect("inject");

        assert!(
            !repo.join(".cursor/mcp.json.agent-teams-backup").exists(),
            "no backup inside a disposable agent-teams worktree"
        );
        let now = fs::read_to_string(repo.join(".cursor/mcp.json")).unwrap();
        assert!(now.contains("agent-teams"), "overwritten in place");
        let _ = fs::remove_dir_all(&root);
    }

    // AC-3 (claude): a staged config OUTSIDE the worktree whose `command` == the
    // resolved sidecar path and `env.AGENT_TEAMS_STATE_DIR` == the app state_root;
    // NO `.git/info/exclude` entry is added (the file is not in the worktree).
    #[test]
    fn claude_config_is_staged_outside_worktree_with_command_and_state_dir() {
        let root = scratch("claude");
        let repo = root.join("worktree");
        let staged = root.join("staged-hooks");
        let state = root.join("app-state");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&staged).unwrap();
        // make the worktree a git repo so an erroneous exclude would be observable
        fs::create_dir_all(repo.join(".git/info")).unwrap();
        // seed the real templates into the staged dir (mirrors how the app stages them)
        fs::copy(
            templates_dir().join("claude-mcp.tmpl.json"),
            staged.join("claude-mcp.tmpl.json"),
        )
        .unwrap();

        let sidecar = PathBuf::from("/abs/path/to/agent-teams-mcp");
        let cfg = cfg_for("claude", repo.clone(), staged.clone(), state.clone());
        let out = inject_mcp_config(&cfg, InjectHarness::Claude, &sidecar)
            .expect("inject claude")
            .expect("claude returns Some(path)");

        // the staged file is OUTSIDE the worktree
        assert!(
            !out.starts_with(&repo),
            "claude config must be staged outside the worktree, got {}",
            out.display()
        );
        assert_eq!(out, staged.join("ws-claude-claude-mcp.json"));

        // parseable JSON with the resolved command + STATE_DIR env
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&out).unwrap()).expect("parseable JSON");
        let srv = &v["mcpServers"]["agent-teams"];
        assert_eq!(srv["command"], "/abs/path/to/agent-teams-mcp");
        assert_eq!(
            srv["env"]["AGENT_TEAMS_STATE_DIR"],
            state.to_string_lossy().as_ref()
        );
        // enablement slice: the sidecar's provenance (C2) + memory key (C7) + task
        // scope (4c) ride the same proven env block as STATE_DIR.
        assert_eq!(srv["env"]["AGENT_TEAMS_PANE_ID"], "ws-claude-p0");
        assert_eq!(srv["env"]["AGENT_TEAMS_MEMORY_REPO_KEY"], "ws-claude");
        assert_eq!(srv["env"]["AGENT_TEAMS_TASK_SCOPE"], "ws-claude-p0");
        assert!(srv["args"].as_array().unwrap().is_empty());

        // NO exclude entry was added (file is not in the worktree)
        let excl = repo.join(".git/info/exclude");
        let body = fs::read_to_string(&excl).unwrap_or_default();
        assert!(
            !body.contains("claude-mcp"),
            "claude staged file must NOT be added to .git/info/exclude: {body:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    // AC-3 (cursor): writes <worktree>/.cursor/mcp.json with the same shape and
    // appends it to .git/info/exclude exactly once (idempotent on a second call);
    // returns None (cursor discovers it by path).
    #[test]
    fn cursor_config_in_worktree_and_excluded_idempotently() {
        let root = scratch("cursor");
        let repo = root.join("worktree");
        let staged = root.join("staged-hooks");
        let state = root.join("app-state");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&staged).unwrap();
        fs::create_dir_all(repo.join(".git/info")).unwrap();
        fs::copy(
            templates_dir().join("cursor-mcp.tmpl.json"),
            staged.join("cursor-mcp.tmpl.json"),
        )
        .unwrap();

        let sidecar = PathBuf::from("/abs/path/to/agent-teams-mcp");
        let cfg = cfg_for("cursor", repo.clone(), staged.clone(), state.clone());

        let none = inject_mcp_config(&cfg, InjectHarness::Cursor, &sidecar).expect("inject cursor");
        assert!(none.is_none(), "cursor returns None (discovered by path)");

        let written = repo.join(".cursor/mcp.json");
        assert!(written.exists(), "cursor config in the worktree");
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&written).unwrap()).expect("parseable JSON");
        let srv = &v["mcpServers"]["agent-teams"];
        // cursor-agent splits the `command` string on whitespace (claude doesn't),
        // so the bundled `/Applications/Agent Teams.app/…` path with a space makes it
        // `spawn /Applications/Agent` → ENOENT. Wrap in `/bin/sh -c "exec '<path>'"`:
        // the command is space-free (/bin/sh) and sh handles the quoted spaced path.
        assert_eq!(srv["command"], "/bin/sh");
        assert_eq!(srv["args"][0], "-c");
        assert_eq!(srv["args"][1], "exec '/abs/path/to/agent-teams-mcp'");
        assert_eq!(
            srv["env"]["AGENT_TEAMS_STATE_DIR"],
            state.to_string_lossy().as_ref()
        );
        assert_eq!(srv["env"]["AGENT_TEAMS_PANE_ID"], "ws-cursor-p0");
        assert_eq!(srv["env"]["AGENT_TEAMS_MEMORY_REPO_KEY"], "ws-cursor");
        assert_eq!(srv["env"]["AGENT_TEAMS_TASK_SCOPE"], "ws-cursor-p0");

        // excluded exactly once, idempotent on a second inject
        inject_mcp_config(&cfg, InjectHarness::Cursor, &sidecar).expect("inject cursor again");
        let body = fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert_eq!(
            body.matches(".cursor/mcp.json").count(),
            1,
            "exclude entry must be idempotent: {body:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    // commandcode: a project-local `.mcp.json` at the worktree ROOT (auto-discovered),
    // same schema + /bin/sh -c exec wrapper + provenance env as cursor, git-excluded
    // idempotently. The state-blind harness is still a first-class MCP client.
    #[test]
    fn commandcode_config_at_worktree_root_and_excluded_idempotently() {
        let root = scratch("commandcode");
        let repo = root.join("worktree");
        let staged = root.join("staged-hooks");
        let state = root.join("app-state");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&staged).unwrap();
        fs::create_dir_all(repo.join(".git/info")).unwrap();
        fs::copy(
            templates_dir().join("commandcode-mcp.tmpl.json"),
            staged.join("commandcode-mcp.tmpl.json"),
        )
        .unwrap();

        let sidecar = PathBuf::from("/abs/path/to/agent-teams-mcp");
        let written = inject_commandcode_mcp(&repo, &staged, &sidecar, &state, "ws-cc-p0", "ws-cc")
            .expect("inject commandcode");

        // project scope = `.mcp.json` at the worktree ROOT
        assert_eq!(written, repo.join(".mcp.json"));
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&written).unwrap()).expect("parseable JSON");
        let srv = &v["mcpServers"]["agent-teams"];
        // same /bin/sh -c exec wrapper as cursor (spaced bundle path safety)
        assert_eq!(srv["command"], "/bin/sh");
        assert_eq!(srv["args"][0], "-c");
        assert_eq!(srv["args"][1], "exec '/abs/path/to/agent-teams-mcp'");
        // the D56/D60 provenance env block (same channel as claude/cursor/codex)
        assert_eq!(
            srv["env"]["AGENT_TEAMS_STATE_DIR"],
            state.to_string_lossy().as_ref()
        );
        assert_eq!(srv["env"]["AGENT_TEAMS_PANE_ID"], "ws-cc-p0");
        assert_eq!(srv["env"]["AGENT_TEAMS_MEMORY_REPO_KEY"], "ws-cc");
        assert_eq!(srv["env"]["AGENT_TEAMS_TASK_SCOPE"], "ws-cc-p0");

        // excluded exactly once, idempotent on a second inject
        inject_commandcode_mcp(&repo, &staged, &sidecar, &state, "ws-cc-p0", "ws-cc")
            .expect("inject commandcode again");
        let body = fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert_eq!(
            body.matches(".mcp.json").count(),
            1,
            "exclude entry must be idempotent: {body:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    // opencode: writes `opencode.json` (NOT `.mcp.json` — OpenCode ignores that format,
    // anomalyco/opencode#27809). Uses OpenCode's native MCP schema: `"mcp"` key (not
    // `"mcpServers"`), `type: "local"`, `command` as a string array, `environment` (not `env`).
    // Git-excluded idempotently, like commandcode/cursor.
    #[test]
    fn opencode_config_at_worktree_root_and_excluded_idempotently() {
        let root = scratch("opencode");
        let repo = root.join("worktree");
        let staged = root.join("staged-hooks");
        let state = root.join("app-state");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&staged).unwrap();
        fs::create_dir_all(repo.join(".git/info")).unwrap();
        fs::copy(
            templates_dir().join("opencode-mcp.tmpl.json"),
            staged.join("opencode-mcp.tmpl.json"),
        )
        .unwrap();

        let sidecar = PathBuf::from("/abs/path/to/agent-teams-mcp");
        let written =
            inject_opencode_mcp(&repo, &staged, &sidecar, &state, "ws-oc-p0", "ws-oc")
                .expect("inject opencode");

        // project scope = `opencode.json` at the worktree ROOT
        assert_eq!(written, repo.join("opencode.json"));
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&written).unwrap()).expect("parseable JSON");
        // OpenCode schema: "mcp" key (NOT "mcpServers")
        assert!(
            v.get("mcp").is_some(),
            "opencode.json must use 'mcp' key, got: {v}"
        );
        assert!(
            v.get("mcpServers").is_none(),
            "opencode.json must NOT use 'mcpServers' key"
        );
        let srv = &v["mcp"]["agent-teams"];
        // type: "local" for stdio servers
        assert_eq!(srv["type"], "local");
        // command as string array (not separate command + args)
        assert_eq!(srv["command"][0], "/bin/sh");
        assert_eq!(srv["command"][1], "-c");
        assert_eq!(srv["command"][2], "exec '/abs/path/to/agent-teams-mcp'");
        // environment (not env)
        assert_eq!(
            srv["environment"]["AGENT_TEAMS_STATE_DIR"],
            state.to_string_lossy().as_ref()
        );
        assert_eq!(srv["environment"]["AGENT_TEAMS_PANE_ID"], "ws-oc-p0");
        assert_eq!(srv["environment"]["AGENT_TEAMS_MEMORY_REPO_KEY"], "ws-oc");
        assert_eq!(srv["environment"]["AGENT_TEAMS_TASK_SCOPE"], "ws-oc-p0");
        // enabled
        assert_eq!(srv["enabled"], true);

        // excluded exactly once, idempotent on a second inject
        inject_opencode_mcp(&repo, &staged, &sidecar, &state, "ws-oc-p0", "ws-oc")
            .expect("inject opencode again");
        let body = fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert_eq!(
            body.matches("opencode.json").count(),
            1,
            "exclude entry must be idempotent: {body:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    // D70 follow-up: the bug only reproduces in a REAL linked worktree (`.git` is a FILE
    // → `<repo>/.git/info` doesn't exist), which the faked fixtures above mask. Use real
    // `git worktree add` and assert the exclude lands in the COMMON dir (what git reads).
    #[test]
    fn exclude_from_git_is_worktree_aware_writes_common_dir_exclude() {
        let root = scratch("wt-exclude");
        let main = root.join("main");
        fs::create_dir_all(&main).unwrap();
        let git = |args: &[&str], cwd: &std::path::Path| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .expect("git runs");
            assert!(out.status.success(), "git {args:?} failed: {out:?}");
        };
        git(&["init", "-q"], &main);
        git(&["config", "user.email", "t@t"], &main);
        git(&["config", "user.name", "t"], &main);
        fs::write(main.join("README"), "x").unwrap();
        git(&["add", "-A"], &main);
        git(&["commit", "-qm", "init"], &main);
        let wt = root.join("wt");
        git(&["worktree", "add", "-q", wt.to_str().unwrap()], &main);

        // precondition: a linked worktree's `.git` is a FILE (the bug's root cause)
        assert!(
            wt.join(".git").is_file(),
            "linked worktree .git must be a file"
        );
        // OLD behavior would no-op (no `<wt>/.git/info`); NEW lands in the common exclude.
        assert!(
            !wt.join(".git/info").exists(),
            "no per-worktree .git/info dir"
        );

        exclude_from_git(&wt, ".mcp.json").expect("exclude in worktree");
        let common_exclude = main.join(".git/info/exclude");
        let body = fs::read_to_string(&common_exclude).expect("common exclude written");
        assert!(
            body.lines().any(|l| l == ".mcp.json"),
            "pattern must land in the COMMON dir exclude (what git reads): {body:?}"
        );
        // idempotent across a second inject (exact-line dedup)
        exclude_from_git(&wt, ".mcp.json").expect("exclude again");
        let body2 = fs::read_to_string(&common_exclude).unwrap();
        assert_eq!(
            body2.matches(".mcp.json").count(),
            1,
            "idempotent: {body2:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    // 17-01 / AC3: inject_cursor_role renders cursor-role.tmpl.md → writes
    // <worktree>/.cursor/rules/agent-role.mdc containing the persona, and appends the
    // path to .git/info/exclude exactly once (idempotent on a second call).
    #[test]
    fn cursor_role_rule_written_and_excluded_idempotently() {
        let root = scratch("role");
        let repo = root.join("worktree");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(repo.join(".git/info")).unwrap();

        let persona = "You are a SCOUT. Map this repository before any building.";
        let out = inject_cursor_role(&repo, &templates_dir(), persona).expect("inject role");

        // written at the expected project-local rule path
        assert_eq!(out, repo.join(".cursor/rules/agent-role.mdc"));
        assert!(out.exists(), "rule file present in the worktree");
        let body = fs::read_to_string(&out).unwrap();
        assert!(body.contains(persona), "persona is the rule body: {body:?}");
        assert!(
            body.contains("alwaysApply: true"),
            "rule frontmatter applies always"
        );

        // excluded from git exactly once, idempotent on a second call
        inject_cursor_role(&repo, &templates_dir(), persona).expect("inject role again");
        let excl = fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert_eq!(
            excl.matches(".cursor/rules/agent-role.mdc").count(),
            1,
            "rule exclude entry must be idempotent: {excl:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    // BridgeAgent cross-agent comms: the fragment is "" without BridgeAgent (default-off), and the
    // RENDERED real templates (agent-teams + bridgeagent) stay VALID JSON with the correct per-schema
    // shape — the comma placement is the risky part, so parse the actual templates end-to-end.
    #[test]
    fn bridgeagent_fragment_keeps_templates_valid_json_per_schema_and_noops_when_absent() {
        let bridge = std::path::Path::new("/Users/x/.bridgeagent/bin/bridge");

        // default-off: no bin → empty fragment → template renders with ONLY agent-teams, valid JSON.
        assert_eq!(
            bridgeagent_fragment(None),
            "",
            "no bin → empty fragment"
        );

        // helper: render a real template's placeholders + the bridgeagent fragment.
        let render =
            |name: &str, bin: Option<&std::path::Path>| -> serde_json::Value {
                let tmpl = fs::read_to_string(templates_dir().join(name)).unwrap();
                let s = tmpl
                    .replace("{{SIDECAR}}", "/sc")
                    .replace("{{STATE_DIR}}", "/st")
                    .replace("{{PANE_ID}}", "p0")
                    .replace("{{REPO_KEY}}", "rk")
                    .replace("{{TASK_SCOPE}}", "p0")
                    .replace("{{BRIDGEAGENT}}", &bridgeagent_fragment(bin));
                serde_json::from_str(&s)
                    .unwrap_or_else(|e| panic!("{name} not valid JSON: {e}\n{s}"))
            };

        // flat harnesses: agent-teams preserved + bridgeagent = {command, args:[mcp,serve]}.
        for name in [
            "claude-mcp.tmpl.json",
            "cursor-mcp.tmpl.json",
            "commandcode-mcp.tmpl.json",
        ] {
            // no bin → only agent-teams, still valid JSON.
            let off = render(name, None);
            assert!(
                off["mcpServers"]["agent-teams"].is_object(),
                "{name}: agent-teams present"
            );
            assert!(
                off["mcpServers"]["bridgeagent"].is_null(),
                "{name}: no bridgeagent when absent"
            );
            // with bin → both servers, agent-teams untouched.
            let on = render(name, Some(bridge));
            assert!(
                on["mcpServers"]["agent-teams"].is_object(),
                "{name}: agent-teams preserved"
            );
            let ba = &on["mcpServers"]["bridgeagent"];
            assert_eq!(
                ba["command"], "/Users/x/.bridgeagent/bin/bridge",
                "{name}: bridge command"
            );
            assert_eq!(ba["args"][0], "mcp");
            assert_eq!(ba["args"][1], "serve");
            assert!(
                ba.get("transport").is_none(),
                "{name}: flat schema, no transport"
            );
        }
    }
}
