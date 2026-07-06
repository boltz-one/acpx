//! Bundles the Phase 3 request handlers (filesystem, terminal, permission)
//! and a `session/update` notification sink so [`super::handshake::spawn_and_initialize`]
//! can register them on the connection's [`agent_client_protocol::Builder`]
//! before the handshake runs (handlers must be registered before
//! `connect_with`, not after — see that module's docs for why this lives
//! here rather than being added post-hoc by the runtime engine).
//!
//! Every field is optional: a caller that doesn't need e.g. terminal
//! support (like [`crate::runtime::public::probe::probe_runtime`]'s
//! throwaway handshake-only client) leaves it `None`, and the
//! corresponding wire method responds with a clear "not configured" error
//! instead of panicking.

use std::sync::Arc;

use agent_client_protocol::schema::v1::SessionNotification;
use parking_lot::Mutex;

use super::state::{PermissionStats, PermissionStatsHandle};
use crate::filesystem::FilesystemHandlers;
use crate::permissions::{PermissionEscalationEvent, PermissionPolicy, PermissionRequestHandler};
use crate::terminal::TerminalManager;
use crate::types::{NonInteractivePermissionPolicy, PermissionMode};

/// Permission-request handling needs its own mode/policy/handler copy
/// because `session/request_permission` is a distinct wire method from the
/// fs/terminal confirmation gates (which already carry their own copies
/// internally, see [`FilesystemHandlers`]/[`TerminalManager`]).
#[derive(Clone)]
pub struct PermissionRequestWiring {
    pub mode: PermissionMode,
    pub non_interactive_policy: NonInteractivePermissionPolicy,
    pub handler: Option<Arc<dyn PermissionRequestHandler>>,
    /// Gap 1: programmatic permission policy (autoApprove/autoDeny/escalate
    /// rules). `None` = no policy overrides. Threaded from
    /// `AcpRuntimeOptions.permission_policy` (ADR-7: no CLI/file loader).
    pub policy: Option<PermissionPolicy>,
    /// Gap 2 (ADR-8): fire-and-forget escalation audit callback, invoked
    /// (best-effort, panic-isolated in the RPC handler) whenever the
    /// `escalate` policy branch produces an escalation event without an
    /// interactive handler to resolve it.
    pub on_escalation: Option<Arc<dyn Fn(PermissionEscalationEvent) + Send + Sync>>,
    /// Gap 25: shared permission-decision counters, incremented by the
    /// `session/request_permission` RPC handler.
    pub stats: PermissionStatsHandle,
}

impl Default for PermissionRequestWiring {
    fn default() -> Self {
        Self {
            mode: PermissionMode::ApproveReads,
            non_interactive_policy: NonInteractivePermissionPolicy::Deny,
            handler: None,
            policy: None,
            on_escalation: None,
            stats: Arc::new(Mutex::new(PermissionStats::default())),
        }
    }
}

/// Everything [`super::handshake::spawn_and_initialize`] needs to wire the
/// agent-initiated RPCs onto the connection.
#[derive(Clone, Default)]
pub struct ClientRequestHandlers {
    pub filesystem: Option<Arc<FilesystemHandlers>>,
    pub terminal: Option<Arc<TerminalManager>>,
    pub permission: PermissionRequestWiring,
    /// Forwards every `session/update` notification verbatim; the runtime
    /// engine's connected-session owns the receiving end and fans updates
    /// out to whichever turn(s) are listening.
    pub notifications: Option<smol::channel::Sender<SessionNotification>>,
}
