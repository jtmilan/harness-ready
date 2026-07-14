//! Small shared FS helpers for the daemon's durable sibling files.

use std::path::{Path, PathBuf};

/// Atomic-replace write: write the bytes to `<path>.tmp`, then `rename` over `path`.
/// `rename(2)` is atomic on the same volume, so a concurrent reader (the GUI's registry
/// poll, the sidecar's discovery read, a re-attach probe) can never observe a torn /
/// truncated file — which a bare `std::fs::write` (truncate-then-write in place) allows.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp_os = path.as_os_str().to_owned();
    tmp_os.push(".tmp");
    let tmp = PathBuf::from(tmp_os);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_replaces_content_and_leaves_no_tmp() {
        let dir = std::env::temp_dir().join(format!("at-fsutil-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("target.json");
        write_atomic(&path, b"{\"a\":1}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":1}");
        write_atomic(&path, b"{\"a\":2}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":2}");
        assert!(
            !dir.join("target.json.tmp").exists(),
            "tmp file renamed away"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_errors_on_missing_parent() {
        let path = std::env::temp_dir()
            .join(format!("at-fsutil-missing-{}", std::process::id()))
            .join("nope")
            .join("f.json");
        assert!(write_atomic(&path, b"x").is_err());
    }
}
