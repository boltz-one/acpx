//! Session persistence: the versioned on-disk record format, the
//! conversation model (message history + truncation), and the repository
//! (atomic write, index, import/export).
//!
//! Ports the "Core + session persistence" slice of `others/acpx/src/session/`
//! and `others/acpx/src/persisted-key-policy.ts` — see phase-05
//! (`plans/20260705-1718-acpx-to-acp-crate-port/phase-05-session-persistence.md`)
//! for the full requirements/architecture/ADR-5 this module implements.
//!
//! Out of scope (see phase-05's module docs for why): `session/session.ts`
//! (a barrel re-exporting acpx's CLI-only `cli/session/*` surface — not
//! ported, this crate has no CLI), and `appendLegacyHistory`/
//! `LegacyHistoryEntry` (acpx's pre-acpx-session-format migration helper —
//! no predecessor format exists in this port).

pub mod acpx_state;
pub mod config_options;
pub mod conversation_model;
pub mod event_log;
pub mod events;
pub mod export;
pub mod import;
pub mod live_checkpoint;
pub mod mode_preference;
pub mod model_application;
pub mod model_state;
pub mod persisted_key_policy;
pub mod persistence;
pub mod record;
pub mod schema;
pub mod store_options;

pub use acpx_state::SessionAcpxState;
pub use record::{SessionImportedFrom, SessionRecord};
pub use schema::SessionSchemaVersion;
pub use store_options::AcpFileSessionStoreOptions;
