//! Ports `writeSessionRecord`: atomic write-then-rename plus index update.

use std::fs;

use crate::error::{AcpError, Result};
use crate::session::record::SessionRecord;
use crate::session::store_options::{
    AcpFileSessionStoreOptions, atomic_temp_path, ensure_session_dir, safe_session_id,
    session_file_path,
};

use crate::session::persistence::index::{
    load_or_rebuild_session_index, to_session_index_entry, write_session_index,
};
use crate::session::persistence::serialize::serialize_session_record_for_disk;

pub fn write_session_record(
    options: &AcpFileSessionStoreOptions,
    record: &SessionRecord,
) -> Result<()> {
    ensure_session_dir(options).map_err(|err| AcpError::Other(err.into()))?;

    let persisted = serialize_session_record_for_disk(record);
    #[cfg(debug_assertions)]
    {
        if let Err(violation) =
            crate::session::persisted_key_policy::assert_persisted_key_policy(&persisted)
        {
            panic!("{violation}");
        }
    }

    let destination = session_file_path(options, &record.acpx_record_id);
    let temp = atomic_temp_path(&destination);
    let payload =
        serde_json::to_string_pretty(&persisted).map_err(|err| AcpError::Other(err.into()))?;
    fs::write(&temp, format!("{payload}\n")).map_err(|err| AcpError::Other(err.into()))?;
    fs::rename(&temp, &destination).map_err(|err| AcpError::Other(err.into()))?;

    let file_name = safe_session_id(&record.acpx_record_id) + ".json";
    let mut entries =
        load_or_rebuild_session_index(options).map_err(|err| AcpError::Other(err.into()))?;
    entries.retain(|entry| entry.file != file_name);
    entries.push(to_session_index_entry(record, &file_name));
    let files: Vec<String> = entries.iter().map(|entry| entry.file.clone()).collect();
    write_session_index(options, &files, &entries).map_err(|err| AcpError::Other(err.into()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::persistence::repository::resolve_session_record;
    use crate::session::persistence::serialize::test_support::sample_session_record;

    #[test]
    fn write_then_resolve_round_trips_by_exact_id() {
        let dir = tempfile::tempdir().unwrap();
        let options = AcpFileSessionStoreOptions::new(dir.path());
        let record = sample_session_record();
        write_session_record(&options, &record).unwrap();

        let resolved = resolve_session_record(&options, &record.acpx_record_id).unwrap();
        assert_eq!(resolved.acpx_record_id, record.acpx_record_id);
    }

    #[test]
    fn write_creates_index_file() {
        let dir = tempfile::tempdir().unwrap();
        let options = AcpFileSessionStoreOptions::new(dir.path());
        let record = sample_session_record();
        write_session_record(&options, &record).unwrap();
        assert!(options.session_dir().join("index.json").exists());
    }
}
