//! Runtime engine: session lifecycle orchestration behind the public
//! contract (`crate::runtime::public`). Ports `others/acpx/src/runtime/engine/`.

pub mod connected_session;
pub mod lifecycle;
pub mod manager;
mod manager_spawn;
mod manager_support;
pub mod prompt_turn;
pub mod reconnect;
pub mod reuse_policy;
pub mod session_options;
