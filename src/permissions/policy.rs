//! Permission policy rule matching + tool-kind inference from title
//! heuristics. Ports `others/acpx/src/permission-policy.ts` (the policy
//! shape) and the rule-matching half of `others/acpx/src/permissions.ts`
//! (`matchPermissionPolicy`, `findPolicyRule`, `inferToolKind`).

use agent_client_protocol::schema::v1::{RequestPermissionRequest, ToolKind};
use serde::{Deserialize, Serialize};

/// What to do when a policy rule matches a permission request. Ports
/// `PermissionPolicyAction` from `types.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionPolicyAction {
    Approve,
    Deny,
    Escalate,
}

/// Rule lists + default action for auto-approving/denying/escalating
/// permission requests by matched token (tool kind, tool name, or title
/// head). Ports the `PermissionPolicy` shape from `types.ts`; empty rule
/// lists behave like acpx's `undefined` (never match).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PermissionPolicy {
    pub auto_approve: Vec<String>,
    pub auto_deny: Vec<String>,
    pub escalate: Vec<String>,
    pub default_action: Option<PermissionPolicyAction>,
}

/// The result of matching a request against a [`PermissionPolicy`]. Ports
/// `PermissionPolicyMatch` from `permissions.ts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionPolicyMatch {
    pub action: PermissionPolicyAction,
    pub matched_rule: Option<String>,
}

/// `(kind, needles)` table used to infer a [`ToolKind`] from a tool-call
/// title when the agent didn't set `toolCall.kind` explicitly. Ports
/// `TOOL_KIND_TITLE_MATCHERS`.
const TOOL_KIND_TITLE_MATCHERS: &[(ToolKind, &[&str])] = &[
    (ToolKind::Read, &["read", "cat"]),
    (ToolKind::Search, &["search", "find", "grep"]),
    (ToolKind::Edit, &["write", "edit", "patch"]),
    (ToolKind::Delete, &["delete", "remove"]),
    (ToolKind::Move, &["move", "rename"]),
    (ToolKind::Execute, &["run", "execute", "bash"]),
    (ToolKind::Fetch, &["fetch", "http", "url"]),
    (ToolKind::Think, &["think"]),
];

/// Ports `inferToolKind`: prefer the tool call's explicit `kind`, else guess
/// from the title's head token (the text before the first `:`), else `None`
/// when there's no title to guess from at all (as distinct from a title
/// that guesses to `ToolKind::Other`).
pub fn infer_tool_kind(params: &RequestPermissionRequest) -> Option<ToolKind> {
    if let Some(kind) = params.tool_call.fields.kind {
        return Some(kind);
    }

    let title = params
        .tool_call
        .fields
        .title
        .as_deref()?
        .trim()
        .to_lowercase();
    let head = title.split(':').next()?.trim();
    if head.is_empty() {
        return None;
    }

    Some(title_head_tool_kind(head).unwrap_or(ToolKind::Other))
}

fn title_head_tool_kind(head: &str) -> Option<ToolKind> {
    TOOL_KIND_TITLE_MATCHERS
        .iter()
        .find(|(_, needles)| needles.iter().any(|needle| head.contains(needle)))
        .map(|(kind, _)| *kind)
}

/// Ports `isAutoApprovedReadKind`: read/search tool calls auto-approve in
/// the read-or-prompt fallback, regardless of policy/mode.
pub fn is_auto_approved_read_kind(kind: Option<ToolKind>) -> bool {
    matches!(kind, Some(ToolKind::Read) | Some(ToolKind::Search))
}

/// Ports `readToolName`: prefer a `name`/`tool`/`toolName` string property on
/// the tool call's raw input, else the title's head token (split on the
/// first `:` or whitespace, unlike `infer_tool_kind`'s colon-only split).
/// `pub(crate)` because [`super::build_escalation_event`] reuses it for the
/// escalation audit event's `tool_name` field.
pub(crate) fn read_tool_name(params: &RequestPermissionRequest) -> Option<String> {
    if let Some(raw_input) = &params.tool_call.fields.raw_input
        && let Some(object) = raw_input.as_object()
    {
        for key in ["name", "tool", "toolName"] {
            if let Some(value) = object.get(key).and_then(|v| v.as_str()) {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }

    let title = params.tool_call.fields.title.as_deref()?.trim();
    let head = match title.find(|c: char| c == ':' || c.is_whitespace()) {
        Some(idx) => &title[..idx],
        None => title,
    };
    let head = head.trim();
    (!head.is_empty()).then(|| head.to_string())
}

fn normalize_matcher(value: &str) -> String {
    value.trim().to_lowercase()
}

/// Ports `permissionMatchTokens`: the set of normalized strings a policy
/// rule may match against (inferred kind, raw kind, title, title head, tool
/// name).
fn permission_match_tokens(params: &RequestPermissionRequest) -> Vec<String> {
    let mut tokens = std::collections::HashSet::new();

    let kind = infer_tool_kind(params);
    let raw_kind = params.tool_call.fields.kind;
    let title = params.tool_call.fields.title.as_deref().map(str::trim);
    let tool_name = read_tool_name(params);

    for kind in [kind, raw_kind] {
        if let Some(kind) = kind
            && let Ok(value) = serde_json::to_value(kind)
            && let Some(text) = value.as_str()
        {
            tokens.insert(normalize_matcher(text));
        }
    }
    if let Some(title) = title
        && !title.is_empty()
    {
        tokens.insert(normalize_matcher(title));
        let head = match title.find([':', ' ', '\t', '\n', '\r']) {
            Some(idx) => &title[..idx],
            None => title,
        };
        if !head.trim().is_empty() {
            tokens.insert(normalize_matcher(head));
        }
    }
    if let Some(tool_name) = tool_name {
        tokens.insert(normalize_matcher(&tool_name));
    }

    tokens.into_iter().collect()
}

/// Ports `findPolicyRule`: `"*"` matches unconditionally, otherwise a rule
/// matches if it equals (case/whitespace-insensitively) any match token.
fn find_policy_rule(rules: &[String], params: &RequestPermissionRequest) -> Option<String> {
    if rules.is_empty() {
        return None;
    }
    let tokens = permission_match_tokens(params);
    rules
        .iter()
        .find(|rule| {
            let normalized = normalize_matcher(rule);
            normalized == "*" || tokens.contains(&normalized)
        })
        .cloned()
}

/// Ports `matchPermissionPolicy`: autoDeny > autoApprove > escalate >
/// defaultAction, first match wins.
pub fn match_permission_policy(
    params: &RequestPermissionRequest,
    policy: Option<&PermissionPolicy>,
) -> Option<PermissionPolicyMatch> {
    let policy = policy?;

    if let Some(rule) = find_policy_rule(&policy.auto_deny, params) {
        return Some(PermissionPolicyMatch {
            action: PermissionPolicyAction::Deny,
            matched_rule: Some(rule),
        });
    }
    if let Some(rule) = find_policy_rule(&policy.auto_approve, params) {
        return Some(PermissionPolicyMatch {
            action: PermissionPolicyAction::Approve,
            matched_rule: Some(rule),
        });
    }
    if let Some(rule) = find_policy_rule(&policy.escalate, params) {
        return Some(PermissionPolicyMatch {
            action: PermissionPolicyAction::Escalate,
            matched_rule: Some(rule),
        });
    }
    policy.default_action.map(|action| PermissionPolicyMatch {
        action,
        matched_rule: None,
    })
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
