//! Configurable session-storage root.
//!
//! acpx hardcodes its session storage root to
//! `path.join(os.homedir(), ".acpx", "sessions")`. Per phase-05's
//! Implementation Step 11 (and Unresolved Question #7, carried in
//! `plans/20260705-1718-acpx-to-acp-crate-port/plan.md`), the Rust port
//! makes this a runtime configuration point instead: a GPUI desktop app may
//! have its own app-data-directory convention distinct from a
//! dotfile-in-home CLI convention, and that decision doesn't need to block
//! this phase.
//!
//! The field name and one-level-of-nesting shape (`state_dir` ->
//! `state_dir/sessions`) intentionally match acpx's `runtime/public/`
//! `AcpFileSessionStoreOptions.stateDir` / `FileSessionStore.sessionDir`
//! (see `others/acpx/src/runtime/public/file-session-store.ts`) so Phase 4's
//! `AcpSessionStore` trait implementation can wrap this module directly.

use std::path::{Path, PathBuf};

/// Where this crate stores session records, the session index, and
/// event-log NDJSON segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpFileSessionStoreOptions {
    /// Root directory. Session files live under `state_dir/sessions`.
    pub state_dir: PathBuf,
}

impl AcpFileSessionStoreOptions {
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            state_dir: state_dir.into(),
        }
    }

    /// The directory session record `.json` files, `index.json`, and
    /// event-log NDJSON files are written to.
    pub fn session_dir(&self) -> PathBuf {
        self.state_dir.join("sessions")
    }
}

impl Default for AcpFileSessionStoreOptions {
    /// A reasonable, distinctly-named default (deliberately not `.acpx`, to
    /// avoid confusing cross-tool interference if a user has acpx installed
    /// too — see phase-05's Risk Assessment).
    ///
    /// Prefers the platform state directory (`XDG_STATE_HOME` on Linux),
    /// falling back to the platform data directory, then to a temp
    /// directory as a last resort so construction never panics.
    fn default() -> Self {
        let base = dirs::state_dir()
            .or_else(dirs::data_dir)
            .unwrap_or_else(std::env::temp_dir);
        Self::new(base.join("boltz-acpx"))
    }
}

/// Percent-encodes `id` for safe use as a filename segment. Ports acpx's
/// `encodeURIComponent(acpxRecordId)` calls (`safeSessionId` /
/// `sessionFilePath` in `repository.ts` and `event-log.ts`).
pub fn safe_session_id(id: &str) -> String {
    const COMPONENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'~');
    percent_encoding::utf8_percent_encode(id, COMPONENT).to_string()
}

pub fn session_file_path(options: &AcpFileSessionStoreOptions, record_id: &str) -> PathBuf {
    options
        .session_dir()
        .join(format!("{}.json", safe_session_id(record_id)))
}

pub fn ensure_session_dir(options: &AcpFileSessionStoreOptions) -> std::io::Result<()> {
    std::fs::create_dir_all(options.session_dir())
}

/// Builds a same-directory sibling temp-file path for an atomic
/// write-then-rename, matching acpx's `${file}.${pid}.${timestamp}.tmp`
/// pattern (see phase-05's Security Considerations: the temp file must live
/// in the same directory as the destination for the rename to be atomic).
pub fn atomic_temp_path(destination: &Path) -> PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let mut name = destination
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".{pid}.{nanos}.tmp"));
    destination.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_session_id_preserves_uuid_like_ids() {
        assert_eq!(
            safe_session_id("2f5f6e2a-8f2f-4e0a-9c9d-000000000000"),
            "2f5f6e2a-8f2f-4e0a-9c9d-000000000000"
        );
    }

    #[test]
    fn safe_session_id_encodes_path_separators() {
        assert_eq!(safe_session_id("a/b"), "a%2Fb");
    }

    #[test]
    fn session_dir_nests_under_state_dir() {
        let options = AcpFileSessionStoreOptions::new("/tmp/example");
        assert_eq!(options.session_dir(), Path::new("/tmp/example/sessions"));
    }

    #[test]
    fn atomic_temp_path_is_same_directory_sibling() {
        let dest = Path::new("/tmp/example/sessions/abc.json");
        let tmp = atomic_temp_path(dest);
        assert_eq!(tmp.parent(), dest.parent());
        assert!(
            tmp.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("abc.json.")
        );
        assert!(tmp.file_name().unwrap().to_str().unwrap().ends_with(".tmp"));
    }
}
