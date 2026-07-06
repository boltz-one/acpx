//! `AcpRuntimeEvent` + the `session/update` -> event parser.
//!
//! Split (per the workspace's <200-line file guideline) into event *shapes*
//! ([`types`]) and the parser that produces them from a typed
//! `SessionUpdate` ([`parse`]) — see [`parse`]'s module docs for why this
//! port's parsing mechanism differs from acpx's own `events.ts`.

mod parse;
mod types;

pub use parse::parse_session_update;
pub use types::{
    AcpRuntimeAvailableCommand, AcpRuntimeEvent, AcpRuntimeTextStream, AcpRuntimeUsageBreakdown,
    AcpRuntimeUsageCost, AcpSessionUpdateTag,
};
