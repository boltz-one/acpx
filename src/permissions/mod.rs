//! Permission decision engine. Ports `others/acpx/src/permissions.ts`'s
//! decision tree (policy match -> mode match -> read-or-prompt fallback).
//! The stdin-prompt mechanism it used (`permission-prompt.ts`) is replaced
//! wholesale by [`responder::PermissionRequestHandler`] per ADR-6 — see that
//! module's docs.
//!
//! Split into one file per concern (workspace's <200-line guideline):
//! `policy` (rule matching), `escalation` (audit event), `response` (wire
//! response builders), `decision` (out-of-band decision mapping), `resolve`
//! (the tree itself), `confirm` (the fs/terminal yes-no gate), `responder`
//! (ADR-6's async handler).

pub mod confirm;
pub mod decision;
pub mod escalation;
pub mod policy;
pub mod resolve;
pub mod responder;
pub mod response;

pub use confirm::confirm_action;
pub use decision::{PermissionDecisionClass, classify_permission_decision, decision_to_response};
pub use escalation::PermissionEscalationEvent;
pub use policy::{
    PermissionPolicy, PermissionPolicyAction, PermissionPolicyMatch, infer_tool_kind,
    is_auto_approved_read_kind, match_permission_policy,
};
pub use resolve::{resolve_permission_request, resolve_permission_request_with_details};
pub use responder::{
    ChannelPermissionRequestHandler, PermissionDecision, PermissionRequestEnvelope,
    PermissionRequestHandler, PermissionResponder,
};
pub use response::ResolvedPermissionRequest;

use crate::types::PermissionMode;

/// Ports `permissionModeSatisfies`: whether `actual` is at least as
/// permissive as `required` on the `deny-all < approve-reads < approve-all`
/// rank (already encoded in [`PermissionMode`]'s derived `Ord`).
pub fn permission_mode_satisfies(actual: PermissionMode, required: PermissionMode) -> bool {
    actual >= required
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_satisfies_follows_deny_read_all_rank() {
        assert!(permission_mode_satisfies(
            PermissionMode::ApproveAll,
            PermissionMode::ApproveReads
        ));
        assert!(!permission_mode_satisfies(
            PermissionMode::DenyAll,
            PermissionMode::ApproveReads
        ));
        assert!(permission_mode_satisfies(
            PermissionMode::ApproveReads,
            PermissionMode::ApproveReads
        ));
    }
}
