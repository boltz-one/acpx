//! Vendored subprocess spawn/kill primitives.
//!
//! Ported (verbatim, minus unused pieces) from `boltz-util`'s `command.rs`,
//! `process.rs`, `redact.rs`, and `util.rs`'s `set_pre_exec_to_start_new_session`
//! — the workspace crate this project was originally extracted from. Only the
//! surface `acpx` actually uses is vendored here, so the crate depends on
//! nothing outside crates.io: [`command::new_std_command`] and
//! [`process::Child`], plus the session-leader helper and command-string
//! secret redaction they rely on.

pub mod command;
pub mod process;
pub mod redact;

/// Ports `boltz-util`'s `set_pre_exec_to_start_new_session`: on Unix, makes
/// the spawned child a new session leader (`setsid`) so it — and its own
/// descendants — can later be signalled as a single process group. No-op on
/// non-Unix targets.
pub fn set_pre_exec_to_start_new_session(
    command: &mut std::process::Command,
) -> &mut std::process::Command {
    // safety: code in pre_exec must be async-signal-safe.
    // https://man7.org/linux/man-pages/man7/signal-safety.7.html
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    command
}
