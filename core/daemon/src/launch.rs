//! The launch-mechanism surface — a trait + two impls.
//!
//! ## Decision (D45 ratified)
//!
//! **A1 — launchd socket-activation** wins. See `08-D45-DECISION.md` for the full
//! justification. The short version: idle-shutdown (AC-4) and auto-restart after
//! idle exit CANNOT coexist under A2 (double-fork + self-bind), but they come for
//! free under A1 because launchd permanently owns the listening socket — the daemon
//! can exit when zero panes are live and the next incoming connection causes launchd
//! to start it again, inheriting the same bound socket.
//!
//! ## Module layout
//!
//! * [`LaunchdSocketActivation`] — the **production** impl. Calls
//!   `launch_activate_socket(3)` (macOS launchd FFI) to inherit the bound fd from
//!   launchd, then wraps it in a [`UnixListener`].  Only meaningful when the binary
//!   is launched by launchd (i.e. the plist is installed and socket-activated). In
//!   any other context it returns a clear error.
//!
//! * [`DoubleFork`] — the **dev / test fallback** impl.  D45 = A1 wins in
//!   production; A2 self-bind is kept here as a named escape hatch for developers
//!   who want to run the daemon directly (`cargo run`, manual launch) without
//!   installing the plist.  It delegates to [`bind_listener_at_path`] so the same
//!   socket-path SSOT is used.
//!
//! * [`bind_listener_at_path`] / [`adopt_fd_as_listener`] — the composable
//!   primitives underneath both impls.  Both are pure (no FFI, no launchd), so they
//!   land and unit-test now.
//!
//! ## macOS-only note
//!
//! `launch_activate_socket` is a macOS-only symbol (from `<launch.h>`). The FFI
//! call inside [`LaunchdSocketActivation::acquire_listener`] is guarded by
//! `#[cfg(target_os = "macos")]`; on other targets it returns a clear
//! `Unsupported` error so the crate still compiles.

use std::ffi::CString;
use std::os::unix::io::{FromRawFd, RawFd};
use std::os::unix::net::UnixListener;
use std::path::Path;

// ---- macOS FFI: launch_activate_socket(3) ----
//
// Signature from <launch.h>:
//   int launch_activate_socket(const char *name, int **fds, size_t *cnt);
// Returns 0 on success. Non-zero is an errno-style error code.
// Caller MUST free() the fds array on both success and failure paths.
//
// The symbol lives in libSystem.B.dylib (always linked on macOS), so no explicit
// #[link(name = "...")] attribute is needed.
#[cfg(target_os = "macos")]
extern "C" {
    fn launch_activate_socket(
        name: *const libc::c_char,
        fds: *mut *mut libc::c_int,
        cnt: *mut libc::size_t,
    ) -> libc::c_int;
}

/// How the daemon comes to own its listening socket. The rest of the daemon is
/// written against this trait so the launchd-vs-double-fork decision touches only
/// the `acquire_listener` body, never the lifecycle or the request handling.
pub trait LaunchMechanism {
    /// Short stable name for logs / diagnostics (`"launchd"` | `"double-fork"`).
    fn name(&self) -> &'static str;

    /// Obtain the daemon's listening socket. Both concrete bodies are implemented:
    /// [`LaunchdSocketActivation`] adopts the socket-activation fd via
    /// `launch_activate_socket(3)`, and [`DoubleFork`] self-binds at
    /// [`agent_teams_core::socket_path`] derived from `state_root`.
    fn acquire_listener(&self, state_root: &Path) -> std::io::Result<UnixListener>;
}

/// **A1 — launchd socket activation (D45 winner).** `launchd` permanently owns and
/// binds the listening socket (via `SockPathName` in the plist). When the first
/// client connects, launchd starts the daemon and hands it the already-bound fd.
/// The daemon calls `launch_activate_socket(3)` to receive that fd and wraps it in
/// a [`UnixListener`].
///
/// # Errors
///
/// * If the daemon was NOT started by launchd (e.g. `cargo run` in dev) or if the
///   plist has no `Sockets/Listeners` entry, `launch_activate_socket` returns a
///   non-zero error code → `Err(Other)` with a diagnostic message.
/// * If launchd reports success but returns zero fds (misconfigured plist) → `Err`.
/// * On non-macOS platforms → `Err(Unsupported)`.
///
/// # Use in production
///
/// The daemon binary should never call this unless it was launched by launchd
/// (i.e. the plist is installed via `install-app.sh`). For dev, use
/// [`DoubleFork`] or run with `--dev` to self-bind via [`bind_listener_at_path`].
#[derive(Debug, Default, Clone, Copy)]
pub struct LaunchdSocketActivation;

impl LaunchMechanism for LaunchdSocketActivation {
    fn name(&self) -> &'static str {
        "launchd"
    }

    fn acquire_listener(&self, _state_root: &Path) -> std::io::Result<UnixListener> {
        // The Sockets dict key in the plist (see install.rs / generate_daemon_plist).
        // MUST match the key used when generating the plist — currently "Listeners".
        const SOCKET_NAME: &str = "Listeners";

        #[cfg(target_os = "macos")]
        {
            // Build the null-terminated socket name string.
            let name_cstr = CString::new(SOCKET_NAME).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "socket name contains interior null byte (should never happen)",
                )
            })?;

            let mut fds: *mut libc::c_int = std::ptr::null_mut();
            let mut cnt: libc::size_t = 0;

            // SAFETY: `launch_activate_socket` is a stable macOS system call.
            // - `name_cstr` is a valid NUL-terminated C string; its pointer is valid
            //   for the duration of the call.
            // - `&mut fds` and `&mut cnt` are valid out-param pointers; launchd
            //   writes through them on success.
            // - On any return (success or failure) the caller is responsible for
            //   `free(fds)` if `fds` is non-null — the branches below both do this.
            let rc = unsafe {
                launch_activate_socket(name_cstr.as_ptr(), &mut fds as *mut _, &mut cnt as *mut _)
            };

            if rc != 0 {
                // Free the array if launchd allocated it even on failure (defensive;
                // Apple's docs are silent on this, but free(NULL) is always safe).
                if !fds.is_null() {
                    // SAFETY: `fds` was set by launchd; freeing it is our contract.
                    unsafe { libc::free(fds as *mut libc::c_void) };
                }
                return Err(std::io::Error::other(format!(
                    "launch_activate_socket(\"{SOCKET_NAME}\") failed with rc={rc}. \
                     Is the daemon running under launchd with a Sockets plist? \
                     Use DoubleFork (--dev) when running outside launchd.",
                )));
            }

            if cnt == 0 {
                // Success but no fds — plist has a Sockets dict but zero entries,
                // which is a misconfiguration we cannot recover from.
                if !fds.is_null() {
                    // SAFETY: same contract — free before returning.
                    unsafe { libc::free(fds as *mut libc::c_void) };
                }
                return Err(std::io::Error::other(format!(
                    "launch_activate_socket(\"{SOCKET_NAME}\") succeeded but \
                     returned 0 fds. Check the plist Sockets/Listeners dict.",
                )));
            }

            if cnt > 1 {
                // More than one fd under this key is unusual (plist misconfiguration).
                // We take the first and log a warning; the extras are leaked here but
                // the fds array itself is freed below.  The extras remain open in the
                // process — launchd will reclaim them on next daemon launch.
                eprintln!(
                    "[daemon:launchd] WARNING: launch_activate_socket returned {cnt} fds \
                     under \"{SOCKET_NAME}\"; expected exactly 1. Using fd[0] and ignoring \
                     the rest. Check the plist Sockets/Listeners dict.",
                );
            }

            // Read the first (and expected-only) fd BEFORE freeing the array.
            // SAFETY: `fds` is a valid pointer and `cnt >= 1` (checked above).
            let raw_fd: RawFd = unsafe { *fds };

            // SAFETY: `fds` is a malloc'd array from launchd; the libc contract
            // requires us to free it.  We have finished reading from it.
            unsafe { libc::free(fds as *mut libc::c_void) };

            // Negative fd from launchd is a defensive check; in practice launchd
            // only writes valid, non-negative fds.
            // SAFETY: raw_fd was delivered by launchd (a trusted kernel-level
            // process); it is a valid, open, listening AF_UNIX socket that we
            // now exclusively own (launchd hands each fd to the daemon only once).
            unsafe { adopt_fd_as_listener(raw_fd) }
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = SOCKET_NAME; // suppress unused-variable warning
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "LaunchdSocketActivation is macOS-only (launchd is a macOS/iOS service). \
                 Use DoubleFork on other platforms.",
            ))
        }
    }
}

/// **A2 — dev / test self-bind fallback.** Binds the socket at
/// [`agent_teams_core::socket_path`]`(state_root)` directly. D45 = A1 wins in
/// production; this impl exists so developers can run the daemon without
/// installing the plist (e.g. `cargo run -- --dev`).
///
/// Note: A2 CANNOT coexist with AC-4 idle-shutdown in production (see D45 § Why
/// A2 self-bind BREAKS AC-4). Use [`LaunchdSocketActivation`] for installed
/// deployments.
#[derive(Debug, Default, Clone, Copy)]
pub struct DoubleFork;

impl LaunchMechanism for DoubleFork {
    fn name(&self) -> &'static str {
        "double-fork"
    }

    fn acquire_listener(&self, state_root: &Path) -> std::io::Result<UnixListener> {
        // Derive the socket path from the SSOT (agent_teams_core::socket_path) —
        // never hardcode or duplicate the path here.
        let socket = agent_teams_core::socket_path(state_root).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "state_root has no parent — cannot derive socket_path for dev self-bind",
            )
        })?;
        bind_listener_at_path(&socket)
    }
}

/// **MF-F launch posture** (08 Sub-build 3 slice 2): pick the launch mechanism by the
/// `--dev` flag and acquire the listener with NO automatic A1→A2 fallback.
///
/// * `dev == false` (production) → **A1** [`LaunchdSocketActivation`]. On ANY A1 error
///   the `Err` is RETURNED so `main` can log + EXIT (fail loud). It MUST NOT silently
///   self-bind — falling back to A2 in production would re-introduce the AC-4 idle-shutdown
///   break D45 ruled out, so the fallback is deliberately absent here.
/// * `dev == true` → **A2** [`DoubleFork`] self-bind, the explicit dev escape hatch.
///
/// Returns the bound listener plus the mechanism's [`LaunchMechanism::name`] for logging.
/// The two paths are mutually exclusive: A2 is reached ONLY via the explicit flag, never
/// as a consequence of an A1 failure.
pub fn acquire_listener_with_posture(
    dev: bool,
    state_root: &Path,
) -> std::io::Result<(UnixListener, &'static str)> {
    if dev {
        let m = DoubleFork;
        m.acquire_listener(state_root).map(|l| (l, m.name()))
    } else {
        // PRODUCTION = A1 ONLY. No fallback: an A1 error propagates so main exits.
        let m = LaunchdSocketActivation;
        m.acquire_listener(state_root).map(|l| (l, m.name()))
    }
}

// ---- Composable socket-acquisition primitives (Phase-08 pure slice) ----
//
// These are the un-gated building blocks underneath the two `acquire_listener`
// bodies. They are pure (stdlib only — no launchd FFI, no daemonize, no clock)
// and can be tested without an installed launchd plist.

/// Bind a fresh Unix-domain listening socket at `path` — creating the parent dir
/// if missing and removing a stale socket file first (a leftover from a crashed
/// prior bind would otherwise fail the bind with `EADDRINUSE`).
///
/// This is the **A2 / dev-mode self-bind** primitive. Under the ratified D45 = A1
/// (launchd socket-activation), the daemon instead INHERITS an already-bound fd via
/// [`adopt_fd_as_listener`], so launchd never calls this. Kept as a tested primitive
/// for the double-fork fallback and for dev/test harnesses that run the daemon
/// outside launchd.
pub fn bind_listener_at_path(path: &Path) -> std::io::Result<UnixListener> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    UnixListener::bind(path)
}

/// Adopt an already-bound, listening Unix-domain socket `raw_fd` (e.g. the one
/// `launchd` hands the daemon under A1 socket-activation via `launch_activate_socket`)
/// as a [`UnixListener`]. Rejects a negative fd; otherwise takes OWNERSHIP of the fd
/// (the returned listener closes it on drop).
///
/// # Safety
/// `raw_fd` must be a valid, open, *listening* Unix-domain socket fd that nothing
/// else owns. That is exactly what `launch_activate_socket` returns. Passing an fd
/// owned elsewhere causes a double-close; passing a non-listening fd makes the first
/// `accept` fail. The FFI call that PRODUCES such an fd lives in the gated
/// `acquire_listener` (it needs launchd at runtime); this adoption step is the pure,
/// testable half and is split out deliberately.
pub unsafe fn adopt_fd_as_listener(raw_fd: RawFd) -> std::io::Result<UnixListener> {
    if raw_fd < 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid file descriptor (negative)",
        ));
    }
    // SAFETY: the caller's contract guarantees a valid, exclusively-owned listening fd.
    Ok(unsafe { UnixListener::from_raw_fd(raw_fd) })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- name() ----

    #[test]
    fn launch_mechanisms_report_their_names() {
        assert_eq!(LaunchdSocketActivation.name(), "launchd");
        assert_eq!(DoubleFork.name(), "double-fork");
    }

    // ---- LaunchdSocketActivation::acquire_listener (negative-fd / error path) ----
    //
    // We CANNOT call the real launch_activate_socket in unit tests (there is no launchd
    // plist installed in CI or a cargo-test environment). We test the ONE branch that
    // fires without launchd: when the process was NOT started by launchd, the call
    // returns a non-zero rc (ESRCH on macOS, or ENOENT), which surfaces as an
    // `ErrorKind::Other` with a diagnostic message.
    //
    // On non-macOS the impl returns `Unsupported` immediately, which we also verify.

    #[test]
    fn launchd_acquire_fails_clearly_outside_launchd() {
        // In a cargo-test process (not launched by launchd), acquire_listener MUST
        // return an error with a message that helps the developer understand the
        // situation. It must NOT panic.
        let err = LaunchdSocketActivation
            .acquire_listener(std::path::Path::new("/tmp/dummy-state"))
            .unwrap_err();

        #[cfg(target_os = "macos")]
        {
            // On macOS without a plist: launch_activate_socket returns non-zero (rc
            // reflects ESRCH/ENOENT/EPERM from launchd), which we map to Other.
            assert_eq!(
                err.kind(),
                std::io::ErrorKind::Other,
                "unexpected kind outside launchd (macOS): {err}"
            );
            let msg = err.to_string();
            // The message must mention the socket name and the actionable hint.
            assert!(
                msg.contains("Listeners"),
                "error must name the socket key: {msg}"
            );
        }

        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(
                err.kind(),
                std::io::ErrorKind::Unsupported,
                "non-macOS must return Unsupported: {err}"
            );
        }
    }

    // ---- DoubleFork::acquire_listener (dev self-bind path) ----

    #[test]
    fn double_fork_acquire_binds_at_socket_path_ssot() {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixStream;

        let base = std::env::temp_dir().join(format!("at-dfork-acquire-{}", std::process::id()));
        // Mirror the real state_root layout: state_root is nested one level under
        // a parent that holds the socket file (per agent_teams_core::socket_path).
        let state_root = base.join("state");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&state_root).unwrap();

        let expected_socket =
            agent_teams_core::socket_path(&state_root).expect("state_root has a parent");

        let listener = DoubleFork
            .acquire_listener(&state_root)
            .expect("dev self-bind");
        let s = expected_socket.clone();
        let h = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 3];
            conn.read_exact(&mut buf).unwrap();
            buf
        });
        // Connect to the derived socket path — proves DoubleFork uses the SSOT.
        UnixStream::connect(&s)
            .expect("connect to socket_path(state_root)")
            .write_all(b"dev")
            .unwrap();
        assert_eq!(&h.join().unwrap(), b"dev");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn double_fork_acquire_errs_on_rootless_state_root() {
        // state_root = "/" has no parent → socket_path returns None → Err.
        let err = DoubleFork
            .acquire_listener(std::path::Path::new("/"))
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("no parent"), "got: {err}");
    }

    // ---- MF-F launch posture: A1-only in production, A2 only via the explicit flag ----

    #[test]
    fn posture_production_is_a1_only_and_does_not_fall_back_to_self_bind() {
        // PRODUCTION posture (dev=false) outside launchd: A1 acquire_listener FAILS, the
        // Err is returned (main exits — fail loud), and CRUCIALLY no self-bind happened —
        // the socket file at socket_path(state_root) must NOT exist (no A2 fallback).
        let base = std::env::temp_dir().join(format!("at-posture-prod-{}", std::process::id()));
        let state_root = base.join("state");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&state_root).unwrap();
        let socket = agent_teams_core::socket_path(&state_root).expect("state_root has a parent");

        let res = acquire_listener_with_posture(false, &state_root);

        #[cfg(target_os = "macos")]
        assert!(
            res.is_err(),
            "A1 outside launchd must fail (no fallback to self-bind)"
        );
        #[cfg(not(target_os = "macos"))]
        assert!(
            res.is_err(),
            "A1 is Unsupported off-macOS — still an error, never a fallback"
        );

        assert!(
            !socket.exists(),
            "production posture must NOT self-bind on A1 failure (exit-not-fallback): {}",
            socket.display()
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn posture_dev_flag_self_binds_via_a2() {
        // DEV posture (dev=true) self-binds via A2 at socket_path(state_root) — A2 is
        // reachable ONLY through the explicit flag.
        use std::io::{Read, Write};
        use std::os::unix::net::UnixStream;
        let base = std::env::temp_dir().join(format!("at-posture-dev-{}", std::process::id()));
        let state_root = base.join("state");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&state_root).unwrap();
        let socket = agent_teams_core::socket_path(&state_root).expect("state_root has a parent");

        let (listener, name) =
            acquire_listener_with_posture(true, &state_root).expect("dev posture self-binds");
        assert_eq!(name, "double-fork", "dev posture uses A2");
        assert!(socket.exists(), "A2 self-bind must create the socket file");

        let s = socket.clone();
        let h = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 2];
            conn.read_exact(&mut buf).unwrap();
            buf
        });
        UnixStream::connect(&s)
            .expect("connect")
            .write_all(b"ok")
            .unwrap();
        assert_eq!(&h.join().unwrap(), b"ok");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_listener_at_path_creates_parent_dirs_and_round_trips() {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixStream;
        let base = std::env::temp_dir().join(format!("at-daemon-bind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let sock = base.join("nested").join("d.sock"); // nested → exercises create_dir_all
        let listener = bind_listener_at_path(&sock).expect("first bind");
        let s = sock.clone();
        let h = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 2];
            conn.read_exact(&mut buf).unwrap();
            buf
        });
        UnixStream::connect(&s)
            .expect("connect")
            .write_all(b"ok")
            .unwrap();
        assert_eq!(&h.join().unwrap(), b"ok");
        // re-bind at the SAME path must succeed (stale socket file removed → no EADDRINUSE).
        let _again = bind_listener_at_path(&sock).expect("re-bind over stale file");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn adopt_fd_rejects_negative_and_round_trips_a_real_fd() {
        use std::io::{Read, Write};
        use std::os::unix::io::IntoRawFd;
        use std::os::unix::net::UnixStream;
        // negative fd → InvalidInput, never a panic.
        let err = unsafe { adopt_fd_as_listener(-1) }.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        // a REAL bound listener, handed over by fd (ownership transferred via into_raw_fd),
        // must adopt + accept a connection — proves the launchd fd-handoff model (A1).
        let base = std::env::temp_dir().join(format!("at-daemon-adopt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let sock = base.join("a.sock");
        let bound = bind_listener_at_path(&sock).expect("bind");
        let fd = bound.into_raw_fd(); // transfer ownership OUT so only the adopted one closes it
        let adopted = unsafe { adopt_fd_as_listener(fd) }.expect("adopt");
        let s = sock.clone();
        let h = std::thread::spawn(move || {
            let (mut conn, _) = adopted.accept().expect("accept");
            let mut buf = [0u8; 2];
            conn.read_exact(&mut buf).unwrap();
            buf
        });
        UnixStream::connect(&s)
            .expect("connect")
            .write_all(b"yo")
            .unwrap();
        assert_eq!(&h.join().unwrap(), b"yo");
        let _ = std::fs::remove_dir_all(&base);
    }
}
