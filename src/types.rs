//! CLI-agnostic subset of `others/acpx/src/types.ts`.
//!
//! Only the types needed by the embeddable client/runtime are ported here.
//! acpx's CLI-only surface (`OUTPUT_*`, `EXIT_CODES`, `OutputFormatter`,
//! `OutputFormat`, queue error detail codes) is out of scope for this crate.

use serde::{Deserialize, Serialize};

/// Permission approval mode. Variant order is significant: it defines the
/// escalation rank acpx enforces via `PERMISSION_MODE_RANK`
/// (`deny-all` < `approve-reads` < `approve-all`), so `#[derive(Ord)]`
/// produces the same comparisons acpx computes by table lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    DenyAll,
    ApproveReads,
    ApproveAll,
}

/// How permission requests are resolved when no interactive prompt is
/// available. Ports `NonInteractivePermissionPolicy` from `types.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NonInteractivePermissionPolicy {
    Deny,
    Fail,
}

/// Whether a runtime may start a brand-new session when the requested one
/// can't be resumed. Ports `SessionResumePolicy` from `types.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionResumePolicy {
    AllowNew,
    SameSessionOnly,
}

/// Behavior when an agent requires authentication acpx doesn't have
/// credentials for. Ports `AuthPolicy` from `types.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthPolicy {
    Skip,
    Fail,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_mode_rank_matches_acpx() {
        assert!(PermissionMode::DenyAll < PermissionMode::ApproveReads);
        assert!(PermissionMode::ApproveReads < PermissionMode::ApproveAll);
    }

    #[test]
    fn permission_mode_serializes_kebab_case() {
        assert_eq!(
            serde_json::to_string(&PermissionMode::ApproveReads).unwrap(),
            "\"approve-reads\""
        );
    }
}
