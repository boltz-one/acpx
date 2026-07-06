//! `AcpSessionStore`/`AcpAgentRegistry`: the two trait-backed extension
//! points contract.ts declares as interfaces (a GPUI app plausibly wants to
//! substitute its own storage/registry), plus [`BuiltInAgentRegistry`], the
//! ready-to-use registry over this crate's own built-in agent table.

use std::collections::HashMap;

use futures::future::BoxFuture;

use crate::session::record::SessionRecord;

/// Ports `AcpSessionStore`. `load`/`save` are modeled as fallible (acpx's
/// `Promise<T>` can reject too; Rust just makes that explicit in the
/// signature) over this crate's own [`SessionRecord`].
pub trait AcpSessionStore: Send + Sync {
    fn load(
        &self,
        session_id: String,
    ) -> BoxFuture<'static, crate::error::Result<Option<SessionRecord>>>;
    fn save(&self, record: SessionRecord) -> BoxFuture<'static, crate::error::Result<()>>;
}

/// Ports `AcpAgentRegistry`.
pub trait AcpAgentRegistry: Send + Sync {
    fn resolve(&self, agent_name: &str) -> String;
    fn list(&self) -> Vec<String>;
}

/// A ready-to-use [`AcpAgentRegistry`] over [`crate::agent_command::registry`]'s
/// built-in table, with optional caller overrides layered on top.
pub struct BuiltInAgentRegistry {
    overrides: Option<HashMap<String, String>>,
}

impl BuiltInAgentRegistry {
    pub fn new(overrides: Option<HashMap<String, String>>) -> Self {
        Self { overrides }
    }
}

impl Default for BuiltInAgentRegistry {
    fn default() -> Self {
        Self::new(None)
    }
}

impl AcpAgentRegistry for BuiltInAgentRegistry {
    fn resolve(&self, agent_name: &str) -> String {
        crate::agent_command::resolve_agent_command(agent_name, self.overrides.as_ref())
    }

    fn list(&self) -> Vec<String> {
        crate::agent_command::list_built_in_agents(self.overrides.as_ref())
    }
}
