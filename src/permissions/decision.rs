//! Mapping between an out-of-band [`PermissionDecision`] and the wire
//! [`RequestPermissionResponse`]/coarse approval class. Ports
//! `decisionToResponse`/`classifyPermissionDecision` from `permissions.ts`.

use agent_client_protocol::schema::v1::{
    PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse,
};

use super::responder::PermissionDecision;
use super::response::{cancelled, pick_option, selected};

fn decision_fallback_order(decision: PermissionDecision) -> Option<[PermissionOptionKind; 2]> {
    match decision {
        PermissionDecision::AllowOnce => Some([
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::AllowAlways,
        ]),
        PermissionDecision::AllowAlways => Some([
            PermissionOptionKind::AllowAlways,
            PermissionOptionKind::AllowOnce,
        ]),
        PermissionDecision::RejectOnce => Some([
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways,
        ]),
        PermissionDecision::RejectAlways => Some([
            PermissionOptionKind::RejectAlways,
            PermissionOptionKind::RejectOnce,
        ]),
        PermissionDecision::Cancel => None,
    }
}

/// Ports `decisionToResponse`: maps an out-of-band [`PermissionDecision`]
/// (e.g. from [`super::PermissionRequestHandler`]) onto whichever concrete
/// `PermissionOption` the agent actually offered, falling back to the
/// paired once/always variant, then to cancellation.
pub fn decision_to_response(
    params: &RequestPermissionRequest,
    decision: PermissionDecision,
) -> RequestPermissionResponse {
    let Some(kinds) = decision_fallback_order(decision) else {
        return cancelled();
    };
    match pick_option(&params.options, &kinds) {
        Some(option) => selected(&option.option_id),
        None => cancelled(),
    }
}

/// A resolved permission response's coarse classification. Ports the return
/// type of `classifyPermissionDecision`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecisionClass {
    Approved,
    Denied,
    Cancelled,
}

/// Ports `classifyPermissionDecision`.
pub fn classify_permission_decision(
    params: &RequestPermissionRequest,
    response: &RequestPermissionResponse,
) -> PermissionDecisionClass {
    let RequestPermissionOutcome::Selected(selected) = &response.outcome else {
        return PermissionDecisionClass::Cancelled;
    };
    let Some(option) = params
        .options
        .iter()
        .find(|option| option.option_id == selected.option_id)
    else {
        return PermissionDecisionClass::Cancelled;
    };
    match option.kind {
        PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways => {
            PermissionDecisionClass::Approved
        }
        _ => PermissionDecisionClass::Denied,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{
        PermissionOption, PermissionOptionId, ToolCallId, ToolCallUpdate, ToolCallUpdateFields,
    };

    fn request() -> RequestPermissionRequest {
        RequestPermissionRequest::new(
            "session-1",
            ToolCallUpdate::new(
                ToolCallId::new("tool-1"),
                ToolCallUpdateFields::new().title("Read: file.txt"),
            ),
            vec![
                PermissionOption::new(
                    PermissionOptionId::new("allow_once"),
                    "Allow",
                    PermissionOptionKind::AllowOnce,
                ),
                PermissionOption::new(
                    PermissionOptionId::new("reject_once"),
                    "Reject",
                    PermissionOptionKind::RejectOnce,
                ),
            ],
        )
    }

    #[test]
    fn classify_decision_matches_selected_option_kind() {
        let params = request();
        let response = selected(&PermissionOptionId::new("allow_once"));
        assert_eq!(
            classify_permission_decision(&params, &response),
            PermissionDecisionClass::Approved
        );
    }

    #[test]
    fn classify_decision_unknown_option_id_is_cancelled() {
        let params = request();
        let response = selected(&PermissionOptionId::new("does-not-exist"));
        assert_eq!(
            classify_permission_decision(&params, &response),
            PermissionDecisionClass::Cancelled
        );
    }

    #[test]
    fn decision_to_response_falls_back_to_paired_kind() {
        let params = request();
        // Only `reject_once` exists; `RejectAlways` should fall back to it
        // per DECISION_FALLBACK_ORDER.
        let response = decision_to_response(&params, PermissionDecision::RejectAlways);
        assert_eq!(
            classify_permission_decision(&params, &response),
            PermissionDecisionClass::Denied
        );
    }

    #[test]
    fn decision_to_response_cancel_is_always_cancelled() {
        let params = request();
        let response = decision_to_response(&params, PermissionDecision::Cancel);
        assert!(matches!(
            response.outcome,
            RequestPermissionOutcome::Cancelled
        ));
    }
}
