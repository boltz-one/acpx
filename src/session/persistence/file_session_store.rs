//! Ports `others/acpx/src/runtime/public/file-session-store.ts`: an
//! `AcpSessionStore` (Phase 4's `runtime::public::contract` trait)
//! implementation over this module's own file-backed repository.
//!
//! Lives here rather than under `runtime/` per phase-04's Implementation
//! Step 11: it's purely a thin adapter over
//! [`super::repository::resolve_session_record`]/[`super::repository::write_session_record`]
//! (atomic write, `assertPersistedKeyPolicy` in debug builds, etc. are
//! already handled there) — only the trait *definition*
//! (`AcpSessionStore`) lives in `runtime::public::contract`.

use std::sync::Arc;

use futures::FutureExt;
use futures::future::BoxFuture;

use crate::error::{AcpError, Result};
use crate::runtime::public::contract::AcpSessionStore;
use crate::session::record::SessionRecord;
use crate::session::store_options::AcpFileSessionStoreOptions;

use super::repository::{resolve_session_record, write_session_record};

/// Ports `FileSessionStore`.
pub struct FileAcpSessionStore {
    options: AcpFileSessionStoreOptions,
}

impl FileAcpSessionStore {
    pub fn new(options: AcpFileSessionStoreOptions) -> Arc<Self> {
        Arc::new(Self { options })
    }
}

impl AcpSessionStore for FileAcpSessionStore {
    /// Ports `FileSessionStore.load`: `Ok(None)` when the record simply
    /// doesn't exist yet (ports the `ENOENT -> undefined` branch); any other
    /// failure (malformed JSON, unreadable directory, ...) propagates.
    fn load(&self, session_id: String) -> BoxFuture<'static, Result<Option<SessionRecord>>> {
        let options = self.options.clone();
        async move {
            match resolve_session_record(&options, &session_id) {
                Ok(record) => Ok(Some(record)),
                Err(AcpError::SessionNotFound { .. }) => Ok(None),
                Err(err) => Err(err),
            }
        }
        .boxed()
    }

    /// Ports `FileSessionStore.save`.
    fn save(&self, record: SessionRecord) -> BoxFuture<'static, Result<()>> {
        let options = self.options.clone();
        async move { write_session_record(&options, &record) }.boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::persistence::serialize::test_support::sample_session_record;

    #[test]
    fn load_missing_session_returns_none_not_error() {
        smol::block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let store = FileAcpSessionStore::new(AcpFileSessionStoreOptions::new(dir.path()));
            let loaded = store.load("does-not-exist".to_string()).await.unwrap();
            assert!(loaded.is_none());
        });
    }

    #[test]
    fn save_then_load_round_trips() {
        smol::block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let store = FileAcpSessionStore::new(AcpFileSessionStoreOptions::new(dir.path()));
            let record = sample_session_record();
            store.save(record.clone()).await.unwrap();
            let loaded = store
                .load(record.acpx_record_id.clone())
                .await
                .unwrap()
                .expect("record should round-trip");
            assert_eq!(loaded.acpx_record_id, record.acpx_record_id);
        });
    }
}
