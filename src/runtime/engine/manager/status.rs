//! `AcpRuntime` status/control surface: capabilities, status snapshot,
//! `session/set_mode` and `session/set_config_option`, persistence, and
//! `doctor`. Split out of `manager/mod.rs` per the workspace's per-file
//! line convention — see that module's docs for the split rationale.

use std::sync::Arc;

use super::AcpRuntime;
use crate::runtime::engine::connected_session::ConnectedSession;
use crate::runtime::engine::manager_support::{runtime_status_from_record, wrap_err};
use crate::runtime::public::contract::{
    AcpRuntimeCapabilities, AcpRuntimeControl, AcpRuntimeDoctorReport, AcpRuntimeHandle,
    AcpRuntimeStatus,
};
use crate::runtime::public::errors::{AcpRuntimeError, AcpRuntimeErrorCode};
use crate::runtime::public::probe::probe_runtime;
use crate::session::config_options::apply_config_options_to_record;
use crate::session::mode_preference::{
    set_desired_config_option, set_desired_mode_id, set_desired_model_id,
};
use crate::session::model_application::current_model_id_from_set_model_response;

impl AcpRuntime {
    /// Ports `getCapabilities` (gap 13). With no handle, returns the
    /// static base capability set unchanged — backward-compatible for
    /// no-handle callers. With a handle, reads `config_option_keys` from
    /// that session's actually-advertised config options: from the live
    /// in-memory record when the session is currently connected (this
    /// phase's ADR — zero extra I/O for the common case), falling back to
    /// `session_store.load()` for a handle that resolves to a
    /// not-currently-connected session. A handle that resolves to nothing
    /// (or advertises no config options) also falls back to the static
    /// list.
    pub async fn get_capabilities(
        &self,
        handle: Option<&AcpRuntimeHandle>,
    ) -> AcpRuntimeCapabilities {
        let base = Self::base_capabilities();
        let Some(handle) = handle else {
            return base;
        };

        let config_options = match self.connected(handle) {
            Ok(connected) => connected
                .record
                .lock()
                .acpx
                .as_ref()
                .and_then(|acpx| acpx.config_options.clone()),
            Err(_) => {
                let record_id = self
                    .resolve_handle_from_runtime_session_name(handle)
                    .acpx_record_id
                    .unwrap_or_else(|| handle.session_key.clone());
                self.options
                    .session_store
                    .load(record_id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|record| record.acpx)
                    .and_then(|acpx| acpx.config_options)
            }
        };

        let Some(config_options) = config_options else {
            return base;
        };

        let mut keys: Vec<String> = Vec::new();
        for option in config_options {
            let id = option.id.0.trim();
            if !id.is_empty() && !keys.iter().any(|existing| existing == id) {
                keys.push(id.to_string());
            }
        }

        if keys.is_empty() {
            return base;
        }

        AcpRuntimeCapabilities {
            config_option_keys: Some(keys),
            ..base
        }
    }

    fn base_capabilities() -> AcpRuntimeCapabilities {
        AcpRuntimeCapabilities {
            controls: vec![
                AcpRuntimeControl::SetMode,
                AcpRuntimeControl::SetConfigOption,
                AcpRuntimeControl::Status,
            ],
            config_option_keys: None,
        }
    }

    /// Ports `getStatus`.
    pub async fn get_status(
        &self,
        handle: &AcpRuntimeHandle,
    ) -> Result<AcpRuntimeStatus, AcpRuntimeError> {
        let connected = self.connected(handle)?;
        let record = connected.record.lock();
        Ok(runtime_status_from_record(&record))
    }

    /// Ports `setMode`.
    pub async fn set_mode(
        &self,
        handle: &AcpRuntimeHandle,
        mode: &str,
    ) -> Result<(), AcpRuntimeError> {
        let connected = self.connected(handle)?;
        connected.set_session_mode(mode).await.map_err(|err| {
            wrap_err(
                AcpRuntimeErrorCode::BackendUnsupportedControl,
                "session/set_mode failed",
                err,
            )
        })?;
        {
            let mut record = connected.record.lock();
            set_desired_mode_id(&mut record, Some(mode));
        }
        self.persist(&connected).await
    }

    /// Ports `setSessionModel` orchestration (previously only reachable
    /// internally via `ConnectedSession::set_session_model`, with no way
    /// for a public caller to also persist the desired-model preference).
    /// Mirrors `manager_spawn`'s initial-model application: folds the
    /// `session/set_config_option` response's config options onto the
    /// record (falling back to the requested `model_id` when the response
    /// carries no model-designated option), so `get_status().models`
    /// reflects the change immediately, before persisting the
    /// desired-model preference.
    pub async fn set_model(
        &self,
        handle: &AcpRuntimeHandle,
        model_id: &str,
        model_config_id: Option<&str>,
    ) -> Result<(), AcpRuntimeError> {
        let connected = self.connected(handle)?;
        let response = connected.set_session_model(model_id).await.map_err(|err| {
            wrap_err(
                AcpRuntimeErrorCode::BackendUnsupportedControl,
                "session/set_model failed",
                err,
            )
        })?;
        {
            let mut record = connected.record.lock();
            let current_model_id = current_model_id_from_set_model_response(
                Some(response.config_options.as_slice()),
                Some(model_id),
            );
            apply_config_options_to_record(&mut record, Some(response.config_options));
            if let (Some(current_model_id), Some(acpx)) = (current_model_id, record.acpx.as_mut()) {
                acpx.current_model_id = Some(current_model_id);
            }
            set_desired_model_id(&mut record, Some(model_id), model_config_id);
        }
        self.persist(&connected).await
    }

    /// Ports `setConfigOption`.
    pub async fn set_config_option(
        &self,
        handle: &AcpRuntimeHandle,
        key: &str,
        value: &str,
    ) -> Result<(), AcpRuntimeError> {
        let connected = self.connected(handle)?;
        connected
            .set_session_config_option(key, value)
            .await
            .map_err(|err| {
                wrap_err(
                    AcpRuntimeErrorCode::BackendUnsupportedControl,
                    "session/set_config_option failed",
                    err,
                )
            })?;
        {
            let mut record = connected.record.lock();
            set_desired_config_option(&mut record, key, Some(value));
        }
        self.persist(&connected).await
    }

    async fn persist(&self, connected: &Arc<ConnectedSession>) -> Result<(), AcpRuntimeError> {
        let snapshot = connected.record.lock().clone();
        self.options
            .session_store
            .save(snapshot)
            .await
            .map_err(|err| {
                wrap_err(
                    AcpRuntimeErrorCode::SessionInitFailed,
                    "failed to persist session record",
                    err,
                )
            })
    }

    /// Ports `doctor`.
    pub async fn doctor(&self) -> AcpRuntimeDoctorReport {
        let report = probe_runtime(&self.options).await;
        AcpRuntimeDoctorReport {
            ok: report.ok,
            code: None,
            message: report.message,
            install_command: None,
            details: report.details,
        }
    }
}
