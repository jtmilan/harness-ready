//! LaunchAgent plist generation for the daemon under the ratified D45 = A1
//! (launchd socket-activation). Pure string generation — no IO, no env — so it
//! lands + unit-tests now; `install-app.sh` (D34) writes the result to
//! `~/Library/LaunchAgents/<LABEL>.plist` and `launchctl bootstrap`s it.
//!
//! The two load-bearing keys (see `08-D45-DECISION.md`):
//! - `RunAtLoad=false` + `KeepAlive=false` → start-on-demand + a CLEAN idle-shutdown
//!   (the AC-4 invariant: the daemon may self-exit when zero panes are live, and the
//!   next connection re-starts it via the launchd-owned socket — this combination is
//!   ONLY possible under socket-activation, which is why A1 won D45).
//! - `Sockets/Listeners/SockPathName` MUST mirror [`agent_teams_core::socket_path`]
//!   (the SSOT). If it drifts, the daemon can't acquire its listener.

use std::path::Path;

/// The daemon's launchd job label (also the plist filename stem).
pub const DAEMON_LABEL: &str = "com.agent-teams.daemon";

/// XML-escape a string for safe embedding in a plist `<string>` value.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Generate the LaunchAgent plist XML for the daemon (A1 socket-activation).
///
/// `daemon_binary_path` is the absolute path to the installed daemon executable;
/// `state_root` is the app's state directory (the `SockPathName` is derived as its
/// sibling via [`agent_teams_core::socket_path`], keeping the SSOT);
/// `stderr_log_path` is the file launchd redirects the daemon's stderr to — pass
/// `None` to omit `StandardErrorPath` (useful in tests or where the caller manages
/// its own log sink).
///
/// Returns `Err` when `state_root` has no parent (e.g. `/`) — the socket path can't
/// be derived, and an install must not write a plist that points nowhere.
pub fn generate_daemon_plist(
    daemon_binary_path: &str,
    state_root: &Path,
    stderr_log_path: Option<&str>,
) -> Result<String, String> {
    let socket = agent_teams_core::socket_path(state_root)
        .ok_or_else(|| "state_root has no parent → cannot derive socket_path".to_string())?;
    let bin = xml_escape(daemon_binary_path);
    let sock = xml_escape(&socket.to_string_lossy());
    // Optional StandardErrorPath block (D45: stable log sink → avoids the
    // daemon's stderr surfacing in Console.app as an unnamed stream).
    let stderr_block = match stderr_log_path {
        Some(p) => format!(
            "  <key>StandardErrorPath</key>\n  <string>{}</string>\n",
            xml_escape(p)
        ),
        None => String::new(),
    };
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{DAEMON_LABEL}</string>
  <key>Program</key>
  <string>{bin}</string>
  <key>RunAtLoad</key>
  <false/>
  <key>KeepAlive</key>
  <false/>
  <key>Sockets</key>
  <dict>
    <key>Listeners</key>
    <dict>
      <key>SockPathName</key>
      <string>{sock}</string>
      <key>SockPathMode</key>
      <integer>384</integer>
    </dict>
  </dict>
{stderr_block}</dict>
</plist>
"#
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn state_root() -> PathBuf {
        // a realistic nested state_root; its parent is the Application Support dir.
        PathBuf::from("/Users/x/Library/Application Support/agent-teams/state")
    }

    #[test]
    fn plist_carries_the_label_and_program() {
        let p = generate_daemon_plist(
            "/Applications/Agent Teams.app/Contents/MacOS/agent-teams-daemon",
            &state_root(),
            None,
        )
        .unwrap();
        assert!(
            p.contains("<string>com.agent-teams.daemon</string>"),
            "label present"
        );
        assert!(
            p.contains("agent-teams-daemon</string>"),
            "program path present"
        );
    }

    #[test]
    fn sockpathname_mirrors_socket_path_ssot() {
        let sr = state_root();
        let expect = agent_teams_core::socket_path(&sr).unwrap();
        let p = generate_daemon_plist("/bin/d", &sr, None).unwrap();
        assert!(
            p.contains(&format!("<string>{}</string>", expect.to_string_lossy())),
            "SockPathName must equal socket_path(state_root): {}",
            expect.display()
        );
    }

    #[test]
    fn socket_activation_keys_are_set_for_idle_shutdown() {
        // RunAtLoad=false + KeepAlive=false is the AC-4 invariant (D45).
        let p = generate_daemon_plist("/bin/d", &state_root(), None).unwrap();
        let run_idx = p.find("<key>RunAtLoad</key>").expect("RunAtLoad key");
        assert!(
            p[run_idx..]
                .trim_start_matches("<key>RunAtLoad</key>")
                .trim_start()
                .starts_with("<false/>"),
            "RunAtLoad=false"
        );
        let ka_idx = p.find("<key>KeepAlive</key>").expect("KeepAlive key");
        assert!(
            p[ka_idx..]
                .trim_start_matches("<key>KeepAlive</key>")
                .trim_start()
                .starts_with("<false/>"),
            "KeepAlive=false"
        );
    }

    #[test]
    fn xml_special_chars_in_the_binary_path_are_escaped() {
        let p = generate_daemon_plist("/opt/a&b/<d>/daemon", &state_root(), None).unwrap();
        assert!(
            p.contains("/opt/a&amp;b/&lt;d&gt;/daemon"),
            "binary path is XML-escaped"
        );
        assert!(!p.contains("a&b/<d>"), "raw special chars must not survive");
    }

    #[test]
    fn no_parent_state_root_is_an_error_not_a_bad_plist() {
        let err = generate_daemon_plist("/bin/d", Path::new("/"), None).unwrap_err();
        assert!(
            err.contains("no parent"),
            "root state_root → Err, never a plist pointing nowhere"
        );
    }

    #[test]
    fn stderr_log_path_appears_in_plist_and_is_xml_escaped() {
        // With log path: StandardErrorPath key is present + value is XML-escaped.
        let p =
            generate_daemon_plist("/bin/d", &state_root(), Some("/path/to/daemon.log")).unwrap();
        assert!(
            p.contains("<key>StandardErrorPath</key>"),
            "StandardErrorPath key present"
        );
        assert!(
            p.contains("<string>/path/to/daemon.log</string>"),
            "log path present"
        );

        // Special chars in the log path are escaped.
        let p2 = generate_daemon_plist("/bin/d", &state_root(), Some("/logs/a&b.log")).unwrap();
        assert!(p2.contains("/logs/a&amp;b.log"), "log path XML-escaped");

        // Without log path: StandardErrorPath key is absent.
        let p3 = generate_daemon_plist("/bin/d", &state_root(), None).unwrap();
        assert!(!p3.contains("StandardErrorPath"), "no log → key absent");
    }
}
