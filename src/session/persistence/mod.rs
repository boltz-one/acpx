//! Session record I/O: parse/serialize the versioned on-disk format, the
//! listing index, and the repository (atomic write, resolve, list, find,
//! prune, close).
//!
//! Ports `others/acpx/src/session/persistence/*.ts` and the non-CLI half of
//! `others/acpx/src/session/persistence.ts`'s barrel re-export.

pub mod file_session_store;
pub mod index;
pub mod parse;
pub mod repository;
pub mod serialize;

pub use file_session_store::FileAcpSessionStore;
pub use parse::parse_session_record;
pub use repository::{
    DEFAULT_HISTORY_LIMIT, FindSessionByDirectoryWalkOptions, FindSessionOptions, PruneOptions,
    PruneResult, absolute_path, close_session, find_git_repository_root, find_session,
    find_session_by_directory_walk, list_sessions, list_sessions_for_agent, normalize_name,
    prune_sessions, resolve_session_record, write_session_record,
};
pub use serialize::serialize_session_record_for_disk;
