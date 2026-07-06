//! Mode/model/config-option replay after a fresh session is created during
//! reconnect. Ports the replay half of
//! `others/acpx/src/runtime/engine/reconnect.ts`
//! (`replayDesiredMode`/`replayDesiredModel`/`replayDesiredConfigOptions`/
//! `replayFreshSessionPreferences`).
//!
//! Replay only runs when reconnect had to fall back to a brand-new backend
//! session (the old one couldn't be resumed/loaded) — a resumed/loaded
//! session already has the user's prior mode/model/config-option choices
//! applied server-side. Any replay step failing surfaces a specific typed
//! [`AcpError`] (`SessionModeReplay`/`SessionModelReplay`/
//! `SessionConfigOptionReplay`) rather than silently proceeding as if the
//! preference had taken effect — this is the exact failure mode Risk
//! Assessment #1 in the phase file warns against.

use agent_client_protocol::schema::v1::SessionId;

use crate::agent_command::model_request::assert_requested_model_supported;
use crate::client::AcpClient;
use crate::error::{AcpError, Result};
use crate::session::config_options::apply_config_options_to_record;
use crate::session::mode_preference::{
    get_desired_config_options, get_desired_mode_id, get_desired_model_id,
};
use crate::session::model_state::advertised_model_state;
use crate::session::record::SessionRecord;

/// Ports `replayDesiredMode`.
async fn replay_desired_mode(
    client: &AcpClient,
    session_id: SessionId,
    desired_mode_id: Option<&str>,
) -> Result<()> {
    let Some(desired_mode_id) = desired_mode_id else {
        return Ok(());
    };
    client
        .set_session_mode(
            session_id,
            agent_client_protocol::schema::v1::SessionModeId::new(desired_mode_id),
        )
        .await
        .map_err(|err| {
            AcpError::SessionModeReplay(format!(
                "failed to replay saved session mode {desired_mode_id}: {err}"
            ))
        })?;
    Ok(())
}

/// Ports `replayDesiredModel`.
async fn replay_desired_model(
    client: &AcpClient,
    session_id: SessionId,
    desired_model_id: Option<&str>,
    record: &mut SessionRecord,
    agent_command: &str,
) -> Result<()> {
    let Some(desired_model_id) = desired_model_id else {
        return Ok(());
    };

    let models = advertised_model_state(record.acpx.as_ref());
    if let Err(err) = assert_requested_model_supported(
        desired_model_id,
        models.as_ref(),
        Some(agent_command),
        true,
    ) {
        log::warn!("[acp] {}", err.message);
    }
    let Some(models) = models else {
        return Ok(());
    };
    if models.current_model_id == desired_model_id {
        return Ok(());
    }

    let config_id = models
        .config_id
        .clone()
        .unwrap_or_else(|| "model".to_string());
    let response = client
        .set_session_config_option(
            session_id,
            agent_client_protocol::schema::v1::SessionConfigId::new(config_id),
            agent_client_protocol::schema::v1::SessionConfigValueId::new(desired_model_id),
        )
        .await
        .map_err(|err| {
            AcpError::SessionModelReplay(format!(
                "failed to replay saved session model {desired_model_id}: {err}"
            ))
        })?;
    apply_config_options_to_record(record, Some(response.config_options));
    Ok(())
}

/// Ports `replayDesiredConfigOptions`.
async fn replay_desired_config_options(
    client: &AcpClient,
    session_id: SessionId,
    desired_config_options: &std::collections::HashMap<String, String>,
    record: &mut SessionRecord,
) -> Result<()> {
    for (config_id, value) in desired_config_options {
        let response = client
            .set_session_config_option(
                session_id.clone(),
                agent_client_protocol::schema::v1::SessionConfigId::new(config_id.clone()),
                agent_client_protocol::schema::v1::SessionConfigValueId::new(value.clone()),
            )
            .await
            .map_err(|err| {
                AcpError::SessionConfigOptionReplay(format!(
                    "failed to replay saved session config option {config_id}: {err}"
                ))
            })?;
        apply_config_options_to_record(record, Some(response.config_options));
    }
    Ok(())
}

/// Ports `replayFreshSessionPreferences`: replays desired mode, then model,
/// then config options (in that order, matching acpx) against a freshly
/// created backend session. On any failure, the caller
/// ([`super::connect_and_load_session`]) is responsible for restoring the
/// record's pre-replay `acp_session_id`/`agent_session_id`/`acpx` state —
/// this function only reports which typed error occurred.
pub async fn replay_fresh_session_preferences(
    client: &AcpClient,
    session_id: SessionId,
    record: &mut SessionRecord,
    agent_command: &str,
) -> Result<()> {
    let desired_mode_id = get_desired_mode_id(record.acpx.as_ref());
    let desired_model_id = get_desired_model_id(record.acpx.as_ref());
    let desired_config_options = get_desired_config_options(record.acpx.as_ref());

    replay_desired_mode(client, session_id.clone(), desired_mode_id.as_deref()).await?;
    replay_desired_model(
        client,
        session_id.clone(),
        desired_model_id.as_deref(),
        record,
        agent_command,
    )
    .await?;
    replay_desired_config_options(client, session_id, &desired_config_options, record).await?;
    Ok(())
}

/// True when `record.acpx` carries any desired-preference state worth
/// replaying at all — lets [`super::connect_and_load_session`] skip the
/// whole replay pass (and its clone-for-rollback overhead) when there is
/// nothing to replay.
pub fn has_preferences_to_replay(record: &SessionRecord) -> bool {
    get_desired_mode_id(record.acpx.as_ref()).is_some()
        || get_desired_model_id(record.acpx.as_ref()).is_some()
        || !get_desired_config_options(record.acpx.as_ref()).is_empty()
}
