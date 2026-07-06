//! Resolving a [`super::SessionExportLookup`] to a [`SessionRecord`].
//!
//! Ports the `loadSessionRecord` half of
//! `others/acpx/src/session/export.ts`.

use std::path::PathBuf;

use crate::error::Result;
use crate::session::persistence::repository::{
    FindSessionOptions, absolute_path, find_session, list_sessions, normalize_name,
};
use crate::session::record::SessionRecord;
use crate::session::store_options::AcpFileSessionStoreOptions;

use super::SessionExportLookup;

pub(super) fn name_matches(session_name: &Option<String>, requested: &Option<String>) -> bool {
    match requested {
        None => session_name.is_none(),
        Some(name) => session_name.as_deref() == Some(name.as_str()),
    }
}

/// Ports `loadSessionRecord`.
pub(super) fn load_session_record(
    options: &AcpFileSessionStoreOptions,
    lookup: &SessionExportLookup,
) -> Result<Option<SessionRecord>> {
    let cwd = absolute_path(&lookup.cwd.clone().unwrap_or_else(|| PathBuf::from(".")));
    let name = normalize_name(lookup.name.as_deref());

    if let Some(agent_command) = &lookup.agent_command {
        if let Some(active) = find_session(
            options,
            &FindSessionOptions {
                agent_command: agent_command.clone(),
                cwd: cwd.clone(),
                name: name.clone(),
                include_closed: false,
            },
        )? {
            return Ok(Some(active));
        }
        let matched = list_sessions(options)?.into_iter().find(|session| {
            session.agent_command == *agent_command
                && session.cwd == cwd.to_string_lossy()
                && name_matches(&session.name, &name)
        });
        return Ok(matched);
    }

    let matches: Vec<SessionRecord> = list_sessions(options)?
        .into_iter()
        .filter(|session| {
            session.cwd == cwd.to_string_lossy() && name_matches(&session.name, &name)
        })
        .collect();
    if matches.len() > 1 {
        return Err(super::lookup_error("multiple sessions match export lookup"));
    }
    Ok(matches.into_iter().next())
}
