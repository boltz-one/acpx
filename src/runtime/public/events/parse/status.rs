//! Maps plan/usage/available-commands/generic status notifications onto
//! [`AcpRuntimeEvent::Status`].

use agent_client_protocol::schema::v1::{AvailableCommandsUpdate, Cost, Meta, Plan};

use super::super::types::{
    AcpRuntimeAvailableCommand, AcpRuntimeEvent, AcpRuntimeUsageBreakdown, AcpRuntimeUsageCost,
};

pub(super) fn plan_status_text(plan: &Plan) -> Option<AcpRuntimeEvent> {
    let first = plan.entries.first()?;
    Some(AcpRuntimeEvent::Status {
        text: format!("plan: {}", first.content),
        tag: Some("plan".to_string()),
        used: None,
        size: None,
        cost: None,
        breakdown: None,
        available_commands: None,
    })
}

pub(super) fn usage_cost(meta_cost: Option<&Cost>) -> Option<AcpRuntimeUsageCost> {
    meta_cost.map(|cost| AcpRuntimeUsageCost {
        amount: Some(cost.amount),
        currency: Some(cost.currency.clone()),
    })
}

/// Reads a `_meta.usage` breakdown if the agent attached one (Claude Code
/// does this; not every adapter does). Ports the relevant half of
/// `normalizeUsageBreakdown`.
pub(super) fn usage_breakdown_from_meta(meta: Option<&Meta>) -> Option<AcpRuntimeUsageBreakdown> {
    let usage = meta?.get("usage")?.as_object()?;
    let read_u64 = |keys: &[&str]| -> Option<u64> {
        keys.iter()
            .find_map(|key| usage.get(*key))
            .and_then(|value| value.as_u64())
    };
    let breakdown = AcpRuntimeUsageBreakdown {
        input_tokens: read_u64(&["inputTokens", "input_tokens"]),
        output_tokens: read_u64(&["outputTokens", "output_tokens"]),
        cached_read_tokens: read_u64(&[
            "cachedReadTokens",
            "cacheReadInputTokens",
            "cache_read_input_tokens",
        ]),
        cached_write_tokens: read_u64(&[
            "cachedWriteTokens",
            "cacheCreationInputTokens",
            "cache_creation_input_tokens",
        ]),
        thought_tokens: read_u64(&["thoughtTokens", "thought_tokens"]),
        total_tokens: read_u64(&["totalTokens", "total_tokens"]),
    };
    (!breakdown.is_empty()).then_some(breakdown)
}

pub(super) fn available_commands_event(update: &AvailableCommandsUpdate) -> AcpRuntimeEvent {
    let available_commands: Vec<AcpRuntimeAvailableCommand> = update
        .available_commands
        .iter()
        .map(|command| AcpRuntimeAvailableCommand {
            name: command.name.clone(),
            description: (!command.description.trim().is_empty())
                .then(|| command.description.clone()),
            has_input: Some(command.input.is_some()),
        })
        .collect();
    let text = if available_commands.is_empty() {
        "available commands updated".to_string()
    } else {
        format!("available commands updated ({})", available_commands.len())
    };
    AcpRuntimeEvent::Status {
        text,
        tag: Some("available_commands_update".to_string()),
        used: None,
        size: None,
        cost: None,
        breakdown: None,
        available_commands: Some(available_commands),
    }
}

pub(super) fn status_event(tag: &str, text: String) -> AcpRuntimeEvent {
    AcpRuntimeEvent::Status {
        text,
        tag: Some(tag.to_string()),
        used: None,
        size: None,
        cost: None,
        breakdown: None,
        available_commands: None,
    }
}
