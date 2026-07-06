//! Session record repository: atomic write, id resolution (exact/suffix/
//! ambiguous), listing, directory-walk discovery, pruning, and close.
//!
//! Ports `others/acpx/src/session/persistence/repository.rs` — the
//! `state_dir`-configurable equivalent of `sessionBaseDir() =
//! path.join(os.homedir(), ".acpx", "sessions")` (see
//! [`crate::session::store_options`] and phase-05's Implementation Step
//! 11). Split across submodules to stay under this crate's per-file line
//! convention: [`write`], [`resolve`], [`find`], [`prune`], [`close`].

mod close;
mod find;
mod prune;
mod resolve;
mod write;

pub use close::close_session;
pub use find::{
    find_git_repository_root, find_session, find_session_by_directory_walk, list_sessions,
    list_sessions_for_agent,
};
pub use prune::{PruneOptions, PruneResult, prune_sessions};
pub use resolve::resolve_session_record;
pub use write::write_session_record;

use std::path::{Path, PathBuf};

use super::index::SessionIndexEntry;
use crate::session::record::SessionRecord;
use crate::session::store_options::AcpFileSessionStoreOptions;

pub const DEFAULT_HISTORY_LIMIT: u32 = 20;

pub struct FindSessionOptions {
    pub agent_command: String,
    pub cwd: PathBuf,
    pub name: Option<String>,
    pub include_closed: bool,
}

pub struct FindSessionByDirectoryWalkOptions {
    pub agent_command: String,
    pub cwd: PathBuf,
    pub name: Option<String>,
    pub boundary: Option<PathBuf>,
}

pub(super) fn load_record_from_file(
    options: &AcpFileSessionStoreOptions,
    file: &str,
) -> Option<SessionRecord> {
    let payload = std::fs::read_to_string(options.session_dir().join(file)).ok()?;
    let value = serde_json::from_str(&payload).ok()?;
    super::parse::parse_session_record(&value)
}

pub(super) fn matches_session_entry(
    entry: &SessionIndexEntry,
    normalized_cwd: &str,
    normalized_name: Option<&str>,
    include_closed: bool,
) -> bool {
    if entry.cwd != normalized_cwd {
        return false;
    }
    if !include_closed && entry.closed {
        return false;
    }
    match normalized_name {
        None => entry.name.is_none(),
        Some(name) => entry.name.as_deref() == Some(name),
    }
}

/// Ports `normalizeName`.
pub fn normalize_name(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Ports `absolutePath` (Node's `path.resolve`): makes `path` absolute
/// against the current working directory and lexically collapses `.`/`..`
/// segments, without resolving symlinks.
pub fn absolute_path(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        use std::path::Component;
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    normalized
}

pub(super) fn is_within_boundary(boundary: &Path, target: &Path) -> bool {
    target.starts_with(boundary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_name_trims_and_rejects_blank() {
        assert_eq!(normalize_name(Some("  demo  ")), Some("demo".to_string()));
        assert_eq!(normalize_name(Some("   ")), None);
        assert_eq!(normalize_name(None), None);
    }

    #[test]
    fn absolute_path_collapses_parent_segments() {
        assert_eq!(
            absolute_path(Path::new("/tmp/a/b/../c")),
            PathBuf::from("/tmp/a/c")
        );
    }
}
