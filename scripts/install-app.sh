#!/usr/bin/env bash
# install-app.sh — install the built Tauri bundle to /Applications and run THAT copy.
#
# WHY: launching the app straight from target/release/bundle/... means every
# `tauri build` rewrites the exact Mach-O you're running -> macOS SIGKILLs it
# ("Killed: 9"). Running a copy in /Applications decouples "the app I use" from
# "the build output", so phase rebuilds (which only touch target/) never
# interrupt the running app.
#
# Usage:
#   bash scripts/install-app.sh            # install latest built bundle, relaunch
#   bash scripts/install-app.sh --build    # `bun tauri build` first, then install
#   bash scripts/install-app.sh --live     # build WITH the `delegate-live` feature (autonomous
#                                          #   workers + flywheel + §6-v2 remediation), then install
#   bash scripts/install-app.sh --testing  # install a SEPARATE "Harness Ready Dev" app (own bundle id
#                                          #   + isolated state dir) ALONGSIDE the stable one — a dev
#                                          #   sandbox to bounce per-commit without touching the app
#                                          #   you use. Implies --live. Quits/relaunches only the dev app.
#   bash scripts/install-app.sh --no-open  # install only, don't relaunch
#
# ⚠️  --live compiles the autonomous-worker / flywheel controller INTO the binary. It is still inert
#     until you ARM it in mcp-config.json (allow_mutations + autonomy_ceiling>=1 + flywheel_apply +
#     flywheel_ship [+ flywheel_remediate for the autonomous HOLD→PASS→PR loop]). The default build
#     ships those code paths as a refused stub; --live makes them runnable behind those gates.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="Harness Ready"        # build-output bundle name (always — tauri.conf productName)
SRC="$REPO/app/src-tauri/target/release/bundle/macos/$APP_NAME.app"

DO_BUILD=0
DO_OPEN=1
LIVE=0
TESTING=0                       # --testing: install an ISOLATED dev app alongside the stable one
for arg in "$@"; do
  case "$arg" in
    --build)   DO_BUILD=1 ;;
    --live)    DO_BUILD=1; LIVE=1 ;;
    --testing) DO_BUILD=1; LIVE=1; TESTING=1 ;;  # dev sandbox: own bundle id + own state dir
    --no-open) DO_OPEN=0 ;;
    *) echo "unknown arg: $arg" >&2; exit 2 ;;
  esac
done

# Harness Ready fork guard: NEVER honor a leaked PRODUCTION state dir. Shells and
# panes descended from the production Agent Teams app inherit AGENT_TEAMS_STATE_DIR
# pointing at prod's state root (documented env-leak footgun). Honoring it here
# would write this fork's dev-updater pointer (and, if the gated daemon were armed,
# its socket path) into PROD's state siblings — poisoning the prod app's "Update
# Now" into dittoing THIS fork's bundle over /Applications/Agent Teams.app. Accept
# an override only when it clearly targets this fork's state tree.
if [[ -n "${AGENT_TEAMS_STATE_DIR:-}" && "${AGENT_TEAMS_STATE_DIR}" != *"harness-ready"* ]]; then
  echo "WARN: ignoring inherited AGENT_TEAMS_STATE_DIR='$AGENT_TEAMS_STATE_DIR'" >&2
  echo "      (not a harness-ready path — likely leaked from the production Agent Teams" >&2
  echo "      environment); falling back to this fork's default state root." >&2
  unset AGENT_TEAMS_STATE_DIR
fi

# --testing installs a SEPARATE "Harness Ready Dev" app: a distinct bundle id (so it coexists with
# AND runs alongside the stable app) + an isolated AGENT_TEAMS_STATE_DIR baked via Info.plist
# LSEnvironment.
#
# CRITICAL: fixed-name siblings (agent-teams-mcp.sock, agent-teams-live.json, mcp-config.json)
# land in the PARENT of state_root. Nest under harness-ready-dev/ (same as scripts/dev.sh) so
# Dev never shares sock/registry/config with stable HR (…/harness-ready/agent-teams). A flat
# …/harness-ready/agent-teams-dev leaf was wrong — both parents were harness-ready/.
DEST_NAME="$APP_NAME"
if [[ "$TESTING" == "1" ]]; then
  DEST_NAME="Harness Ready Dev"
  DEV_BUNDLE_ID="com.jeffrymilan.harnessready.dev"
  DEV_STATE_DIR="$HOME/Library/Application Support/harness-ready-dev/state"
fi
DEST="/Applications/$DEST_NAME.app"

if [[ "$DO_BUILD" == "1" ]]; then
  # Sentry DSN (crash monitoring) is baked at BUILD time via option_env!("AGENT_TEAMS_SENTRY_DSN")
  # in app_lib::init_sentry — NEVER committed. Load it from a gitignored secrets file (or the
  # already-exported env) so the build can pick it up; absent ⇒ Sentry builds inert (default-off).
  if [[ -z "${AGENT_TEAMS_SENTRY_DSN:-}" && -f "$REPO/.sentry-dsn" ]]; then
    AGENT_TEAMS_SENTRY_DSN="$(tr -d '[:space:]' < "$REPO/.sentry-dsn")"
  fi
  export AGENT_TEAMS_SENTRY_DSN="${AGENT_TEAMS_SENTRY_DSN:-}"
  if [[ -n "$AGENT_TEAMS_SENTRY_DSN" ]]; then
    echo "==> Sentry: DSN present → crash monitoring will be ARMED in this build"
  else
    echo "==> Sentry: no DSN (.sentry-dsn / env) → building INERT (no crash reporting)"
  fi
  if [[ "$LIVE" == "1" ]]; then
    echo "==> building WITH delegate-live (bun tauri build -f delegate-live)…"
    ( cd "$REPO/app" && bun tauri build -f delegate-live --bundles app )
  else
    echo "==> building (bun tauri build)…"
    ( cd "$REPO/app" && bun tauri build --bundles app )
  fi
  # Flavor marker (read by the in-app D34 updater): both flavors share this ONE
  # build-output slot, so the updater must be able to tell what's sitting in it —
  # a delegate-live bundle must never be offered to / dittoed over the default
  # install, or vice versa. Stamped into $SRC so the ditto below carries it into
  # /Applications and `codesign --deep` covers it.
  mkdir -p "$SRC/Contents/Resources"
  if [[ "$LIVE" == "1" ]]; then echo "delegate-live" > "$SRC/Contents/Resources/at-flavor"; else echo "default" > "$SRC/Contents/Resources/at-flavor"; fi
fi

if [[ ! -d "$SRC" ]]; then
  echo "ERROR: no built bundle at: $SRC" >&2
  echo "       run with --build, or run \`bun tauri build\` in app/ first." >&2
  exit 1
fi

# Quit ALL running instances first — two instances would collide on the shared
# app-support state_root + WKWebView localStorage (same bundle id).
# Quit only the SAME app we're installing over ("Harness Ready" vs "Harness Ready Dev" are distinct
# bundle ids with isolated state — a --testing install must NOT kill the stable app, and vice versa;
# neither name matches the production "Agent Teams"/"Agent Teams Dev"/"Agent Teams DevTest" apps,
# which this fork's installer must NEVER quit, kill, or touch).
echo "==> quitting any running '$DEST_NAME'…"
osascript -e "quit app \"$DEST_NAME\"" 2>/dev/null || true
sleep 1
pkill -f "$DEST_NAME.app/Contents/MacOS/app" 2>/dev/null || true
sleep 1

echo "==> installing -> $DEST"
rm -rf "$DEST"
ditto "$SRC" "$DEST"                       # bundle-correct copy (preserves perms/xattrs).
                                           # codesign --deep below covers the bundle.

# --testing: rebrand the installed copy into a SEPARATE app (own bundle id + display name) and bake
# the isolated state dir via LSEnvironment. MUST run BEFORE the codesign below — a post-sign plist
# edit invalidates the signature (TCC would then reset every install). LSEnvironment is honored by
# LaunchServices for double-click launches, so the GUI app picks up AGENT_TEAMS_STATE_DIR with no
# shell env. The bundle-id change gives it its own LaunchServices + TCC identity → coexists + runs
# alongside the stable app.
if [[ "$TESTING" == "1" ]]; then
  PL="$DEST/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $DEV_BUNDLE_ID" "$PL"
  /usr/libexec/PlistBuddy -c "Set :CFBundleName Harness Ready Dev" "$PL" 2>/dev/null || true
  /usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName Harness Ready Dev" "$PL" 2>/dev/null \
    || /usr/libexec/PlistBuddy -c "Add :CFBundleDisplayName string Harness Ready Dev" "$PL"
  /usr/libexec/PlistBuddy -c "Delete :LSEnvironment" "$PL" 2>/dev/null || true
  /usr/libexec/PlistBuddy -c "Add :LSEnvironment dict" "$PL"
  /usr/libexec/PlistBuddy -c "Add :LSEnvironment:AGENT_TEAMS_STATE_DIR string $DEV_STATE_DIR" "$PL"
  echo "==> dev variant: bundle id $DEV_BUNDLE_ID · state $DEV_STATE_DIR"
fi

# Provision the stable code-signing identity if it's absent, so
# the preferred-identity branch below actually fires. This closes the c3 cert gap:
# previously install-app.sh only WARNed and asked the operator to run the cert
# script by hand, so on any fresh box the identity never existed → it always fell
# back to ad-hoc → the Screen-Recording TCC grant reset on every rebuild.
# ensure-dev-cert.sh is idempotent (a no-op when the identity already exists).
# GUARDED with `|| echo …`: this script runs under `set -euo pipefail`, so an
# unguarded non-zero exit here would abort the whole install and leave the bundle
# unsigned. Instead we let provisioning fail soft and fall through to the ad-hoc
# fallback below, preserving the hard guarantee that a fresh machine still
# installs and runs.
echo "==> ensuring stable code-signing identity present…"
# Resolve the identity name ONCE and pass it through to provisioning, so an
# AGENT_TEAMS_SIGN_IDENTITY override mints AND signs under the same name (the
# wrapper forwards IDENTITY_NAME to gen-signing-cert.sh). Without this the two
# would drift — provisioning the default name while signing looks for the override.
# NOTE (Harness Ready fork): the default identity name "Agent Teams Dev" is kept
# DELIBERATELY — it is a generic keychain identity inherited from the parent repo
# that already exists on this machine; reusing it avoids minting a second cert.
# The TCC Designated Requirement pins cert leaf + BUNDLE ID, and this fork's
# bundle ids differ from production's, so sharing the cert does not conflate the
# apps' TCC identities. This string names a keychain cert only — never an app.
SIGN_IDENTITY="${AGENT_TEAMS_SIGN_IDENTITY:-Agent Teams Dev}"
IDENTITY_NAME="$SIGN_IDENTITY" bash "$REPO/scripts/ensure-dev-cert.sh" \
  || echo "WARN: identity provisioning failed; will fall back to ad-hoc signing below." >&2

# Re-sign the installed copy. Prefer a STABLE self-signed identity over ad-hoc
# ('-'): with a fixed cert the Designated Requirement pins the certificate + the
# bundle id (both stable across rebuilds) instead of the cdhash, so the
# Screen-Recording TCC grant SURVIVES every `tauri build` + update.
# Ad-hoc re-hashes the bundle each build → cdhash churns → TCC forgets the grant.
# The ensure-dev-cert.sh call above creates the identity when missing; this branch
# then prefers it and falls back to ad-hoc only if provisioning was unavailable.
# NOTE: --deep but deliberately NOT --options runtime — a hardened runtime without
# capture entitlements would BLOCK the screen access we are preserving.
if security find-identity -p codesigning 2>/dev/null | grep -qF "\"$SIGN_IDENTITY\""; then
  echo "==> signing with stable identity: \"$SIGN_IDENTITY\" (TCC grants persist across updates)"
  # NON-FATAL: a stable-identity sign can still fail on a headless box (no GUI to click
  # the one-time keychain-ACL "Always Allow" when KEYCHAIN_PASSWORD wasn't exported).
  # Under `set -euo pipefail` an UNGUARDED failure here would abort the whole install and
  # leave the bundle unsigned — so on failure we fall through to ad-hoc. The install ALWAYS
  # lands a signature (the hard "fresh/headless machine must still install + run" guarantee).
  if ! codesign --force --deep -s "$SIGN_IDENTITY" "$DEST"; then
    echo "WARN: signing with \"$SIGN_IDENTITY\" failed (headless / keychain-ACL prompt declined)." >&2
    echo "      Falling back to ad-hoc so the install still completes." >&2
    codesign --force --deep -s - "$DEST"   # ad-hoc fallback — never leave the bundle unsigned
  fi
else
  echo "WARN: no \"$SIGN_IDENTITY\" code-signing identity found." >&2
  echo "      ensure-dev-cert.sh could not provision it (see above); Screen-Recording" >&2
  echo "      TCC grants will reset on each rebuild. Falling back to ad-hoc for now." >&2
  codesign --force --deep -s - "$DEST"     # ad-hoc fallback
fi
xattr -dr com.apple.quarantine "$DEST"     # clear Gatekeeper quarantine (recursive)

VER="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$DEST/Contents/Info.plist" 2>/dev/null || echo '?')"
echo "==> installed v$VER"

# ── Phase 08: daemon LaunchAgent plist install (A1 socket-activation, D45) ──
#
# Generates and loads ~/Library/LaunchAgents/com.harness-ready.daemon.plist so the
# daemon binary is start-on-demand (launchd socket-activation). Key invariants:
#   RunAtLoad=false  → daemon ONLY starts when a connection arrives on the socket.
#   KeepAlive=false  → launchd does NOT restart on exit; idle-exit = intentional.
#   SockPathName     → MUST match agent_teams_core::socket_path(state_root), i.e.
#                      <state_root-parent>/agent-teams-mcp.sock, the shared UDS.
#   StandardErrorPath → stable log sink at ~/Library/Logs/harness-ready/daemon.log.
#
# This section is idempotent: bootout (unload) is best-effort (no-op if absent),
# then bootstrap (re)loads from the newly-generated plist. A stale plist is
# therefore never left pointing at an old binary.
#
# Skip for --testing: the Dev app has an isolated AGENT_TEAMS_STATE_DIR baked
# into its Info.plist; the daemon plist must derive its SockPathName from the
# SAME state dir. Generating a daemon plist for the dev variant here would
# conflict if the prod plist is already loaded. A separate dev-daemon plist is
# out of scope (daemon is a prod-path feature; dev smoke-testing uses the stub
# main.rs which exits immediately).
_DAEMON_BINARY="$DEST/Contents/MacOS/agent-teams-daemon"
if [[ "$TESTING" == "1" ]]; then
  : # --testing dev variant: no daemon plist (see the note above)
elif [[ ! -x "$_DAEMON_BINARY" || "${AGENT_TEAMS_DAEMON_LAUNCHAGENT:-0}" != "1" ]]; then
  # GATED-OFF (security review). As of 08-T9 the daemon binary IS bundled in
  # tauri.conf externalBin (so the packaged AC-1..6 GUI-verify is runnable). Bundling is
  # NOT un-gating: registration is now DECOUPLED from mere binary-PRESENCE. The old gate
  # was `[[ ! -x "$_DAEMON_BINARY" ]]`, which was only ever true because the binary was
  # absent — so the moment the binary shipped it would have silently flipped TRUE and
  # auto-registered the LaunchAgent. On `launchctl bootstrap`, launchd binds SockPathName
  # (the SAME agent-teams-mcp.sock the running app's MCP server binds → a socket-ownership
  # race). To keep production INERT, registration now ALSO requires an explicit,
  # security-reviewed opt-in: AGENT_TEAMS_DAEMON_LAUNCHAGENT=1. Default installs land HERE
  # and skip — the daemon stays gated OFF at the launchd layer (no plist, no bootstrap, no
  # socket bind), exactly as before, even though the Mach-O is now present in the bundle.
  # Flipping the opt-in is the deliberate role-inversion/socket-handover enablement (still
  # owed a security review of the live PTY-ownership transfer, design Q4).
  echo "==> daemon LaunchAgent registration gated OFF (set AGENT_TEAMS_DAEMON_LAUNCHAGENT=1 to enable) — skipping"
else
  # AGENT_TEAMS_STATE_DIR (default ~/Library/Application Support/harness-ready/agent-teams) IS
  # state_root — the SAME value the app's default_state_root() (app/src-tauri/src/lib.rs)
  # and the daemon's resolve_state_root() (core/daemon/src/main.rs) both resolve. Do NOT
  # append /agent-teams: that would double-nest state_root and place the socket INSIDE it
  # (the inside path is wiped on every app launch — see state_sibling()).
  _DAEMON_STATE_ROOT="${AGENT_TEAMS_STATE_DIR:-$HOME/Library/Application Support/harness-ready/agent-teams}"
  # Socket path mirrors agent_teams_core::socket_path EXACTLY:
  #   socket_path(state_root) = state_root.parent()/agent-teams-mcp.sock
  # i.e. a SIBLING of state_root, one level up (NOT inside it). This is the same UDS the
  # running app's MCP server binds and the daemon_client dialer reaches.
  _DAEMON_SOCK_PATH="$(dirname "$_DAEMON_STATE_ROOT")/agent-teams-mcp.sock"
  # Harness Ready fork: label + log dir are FORK-PRIVATE. Production Agent Teams
  # registers ~/Library/LaunchAgents/com.agent-teams.daemon.plist — reusing that
  # label here would bootout/overwrite the PRODUCTION plist. The socket filename
  # (agent-teams-mcp.sock) stays: it is the agent_teams_core::socket_path SSOT and
  # already isolated by the fork-private state root's parent (harness-ready/).
  _DAEMON_LABEL="com.harness-ready.daemon"
  _DAEMON_PLIST_DIR="$HOME/Library/LaunchAgents"
  _DAEMON_PLIST="$_DAEMON_PLIST_DIR/$_DAEMON_LABEL.plist"
  _DAEMON_LOG_DIR="$HOME/Library/Logs/harness-ready"
  _DAEMON_LOG="$_DAEMON_LOG_DIR/daemon.log"

  # The socket's parent (state_root's parent, …/harness-ready/) is NOT pre-existing the way
  # Application Support/ is — launchd needs it present to bind SockPathName.
  mkdir -p "$_DAEMON_PLIST_DIR" "$_DAEMON_LOG_DIR" "$(dirname "$_DAEMON_SOCK_PATH")"

  echo "==> installing daemon LaunchAgent plist (D45 A1 socket-activation)…"
  cat > "$_DAEMON_PLIST" <<DAEMON_PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>$_DAEMON_LABEL</string>
  <key>Program</key>
  <string>$_DAEMON_BINARY</string>
  <key>RunAtLoad</key>
  <false/>
  <key>KeepAlive</key>
  <false/>
  <key>Sockets</key>
  <dict>
    <key>Listeners</key>
    <dict>
      <key>SockPathName</key>
      <string>$_DAEMON_SOCK_PATH</string>
      <key>SockPathMode</key>
      <integer>384</integer>
    </dict>
  </dict>
  <key>StandardErrorPath</key>
  <string>$_DAEMON_LOG</string>
</dict>
</plist>
DAEMON_PLIST_EOF

  # Verify the plist is well-formed before registering it. plutil exits non-zero
  # and prints an error to stderr on malformed XML; -lint is the fastest check mode.
  if ! plutil -lint "$_DAEMON_PLIST" >/dev/null 2>&1; then
    echo "ERROR: generated daemon plist is malformed — launchd registration skipped." >&2
    echo "       Check $_DAEMON_PLIST for XML errors." >&2
  else
    # Unload any previous job (safe no-op if not loaded — launchd returns an error
    # we intentionally suppress). Then bootstrap the new plist so a stale label is
    # never left pointing at an old binary.
    launchctl bootout "gui/$(id -u)" "$_DAEMON_PLIST" 2>/dev/null || true
    if launchctl bootstrap "gui/$(id -u)" "$_DAEMON_PLIST"; then
      echo "==> daemon LaunchAgent registered: $_DAEMON_LABEL"
      echo "    binary:  $_DAEMON_BINARY"
      echo "    socket:  $_DAEMON_SOCK_PATH"
      echo "    log:     $_DAEMON_LOG"
      echo "    (daemon starts on-demand when a connection arrives; idle-exits after)"
    else
      echo "WARN: launchctl bootstrap failed for $_DAEMON_LABEL." >&2
      echo "      The plist is saved at $_DAEMON_PLIST — load manually:" >&2
      echo "        launchctl bootstrap gui/\$(id -u) \"$_DAEMON_PLIST\"" >&2
    fi
  fi
fi

# Seed the dev-updater pointer: a sibling of state_root (survives the startup
# wipe) naming the dev TARGET bundle. The running /Applications app reads this,
# notices when a fresh `tauri build` lands, and offers a one-click in-app update
# (no terminal needed) — without the rebuild ever killing the running app.
# --testing seeds the DEV app's own pointer (agent-teams-dev-dev-source), not the stable one's.
if [[ "$TESTING" == "1" ]]; then
  STATE_DIR="$DEV_STATE_DIR"
else
  STATE_DIR="${AGENT_TEAMS_STATE_DIR:-$HOME/Library/Application Support/harness-ready/agent-teams}"
fi
DEV_SOURCE="$(dirname "$STATE_DIR")/$(basename "$STATE_DIR")-dev-source"
mkdir -p "$(dirname "$DEV_SOURCE")"
printf '%s\n' "$SRC" > "$DEV_SOURCE"
echo "==> dev-updater pointer -> $DEV_SOURCE"

if [[ "$DO_OPEN" == "1" ]]; then
  echo "==> launching $DEST"
  open "$DEST"
fi
echo "done. Use the /Applications copy from now on — phase rebuilds won't touch it."
echo "     After each \`bun tauri build\`, the running app shows an 'Update Now' card."
