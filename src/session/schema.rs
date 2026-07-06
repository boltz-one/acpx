//! Session-record schema-version tag.
//!
//! Ports the versioning half of `others/acpx/src/types.ts`'s
//! `SESSION_RECORD_SCHEMA = "acpx.session.v1"` constant, adapted per ADR-5
//! (see `plans/20260705-1718-acpx-to-acp-crate-port/phase-05-session-persistence.md`)
//! to a Rust enum instead of a bare string: an unrecognized future schema
//! version fails to deserialize *explicitly* (`serde`'s "unknown variant"
//! error) rather than silently coercing into the current struct shape.
//!
//! Not byte-compatible with acpx's own `"acpx.session.v1"` tag — this is a
//! new, namespaced on-disk format, not a drop-in replacement for acpx's
//! session files.

use serde::{Deserialize, Serialize};

/// The persisted `schema` tag on every [`super::record::SessionRecord`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionSchemaVersion {
    #[serde(rename = "boltz-acpx.session.v1")]
    V1,
}

impl SessionSchemaVersion {
    /// The exact string written to disk for this variant. Kept as a
    /// standalone accessor (rather than only relying on `serde`) so
    /// [`super::persistence::parse`]'s schema-sniffing pre-pass can compare
    /// against it without round-tripping through `serde_json`.
    pub const fn as_str(self) -> &'static str {
        match self {
            SessionSchemaVersion::V1 => "boltz-acpx.session.v1",
        }
    }
}

impl Default for SessionSchemaVersion {
    fn default() -> Self {
        Self::V1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_to_namespaced_tag() {
        assert_eq!(
            serde_json::to_string(&SessionSchemaVersion::V1).unwrap(),
            "\"boltz-acpx.session.v1\""
        );
    }

    #[test]
    fn unrecognized_schema_tag_fails_to_deserialize() {
        let err = serde_json::from_str::<SessionSchemaVersion>("\"acpx.session.v1\"").unwrap_err();
        assert!(err.to_string().contains("unknown variant"));
    }

    #[test]
    fn as_str_matches_serde_rename() {
        assert_eq!(
            serde_json::to_string(&SessionSchemaVersion::V1).unwrap(),
            format!("\"{}\"", SessionSchemaVersion::V1.as_str())
        );
    }
}
