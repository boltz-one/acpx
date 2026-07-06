//! Response-building helpers shared by `resolve.rs` and `decision.rs`: both
//! turn a chosen [`PermissionOption`] (or the lack of one) into a wire
//! [`RequestPermissionResponse`]. Ports the small `selected`/`cancelled`
//! helpers from `permissions.ts`.

use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionId, PermissionOptionKind, RequestPermissionOutcome,
    RequestPermissionResponse, SelectedPermissionOutcome,
};

use super::escalation::PermissionEscalationEvent;

/// Outcome of resolving a permission request, carrying the audit-trail
/// escalation event alongside the wire response so callers (Phase 4's
/// runtime) can log/surface it without re-deriving it from the response.
/// Ports `ResolvedPermissionRequest` from `permissions.ts`.
#[derive(Debug, Clone)]
pub struct ResolvedPermissionRequest {
    pub response: RequestPermissionResponse,
    pub escalation: Option<PermissionEscalationEvent>,
}

pub(super) fn selected(option_id: &PermissionOptionId) -> RequestPermissionResponse {
    RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
        SelectedPermissionOutcome::new(option_id.clone()),
    ))
}

pub(super) fn cancelled() -> RequestPermissionResponse {
    RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
}

pub(super) fn pick_option<'a>(
    options: &'a [PermissionOption],
    kinds: &[PermissionOptionKind],
) -> Option<&'a PermissionOption> {
    kinds
        .iter()
        .find_map(|kind| options.iter().find(|option| option.kind == *kind))
}

pub(super) fn selected_or_first(
    options: &[PermissionOption],
    allow_option: Option<&PermissionOption>,
) -> ResolvedPermissionRequest {
    let response = match allow_option.or_else(|| options.first()) {
        Some(option) => selected(&option.option_id),
        None => cancelled(),
    };
    ResolvedPermissionRequest {
        response,
        escalation: None,
    }
}

pub(super) fn selected_or_cancelled(
    option: Option<&PermissionOption>,
) -> ResolvedPermissionRequest {
    ResolvedPermissionRequest {
        response: option
            .map(|o| selected(&o.option_id))
            .unwrap_or_else(cancelled),
        escalation: None,
    }
}
