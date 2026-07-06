//! Ported from `boltz-util`'s `command.rs` — only `new_std_command` (the sole
//! entry point `acpx` uses; the async `Command` wrapper is not vendored).

use std::ffi::OsStr;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000_u32;

/// A `std::process::Command` for `program`, with `CREATE_NO_WINDOW` set on
/// Windows so spawning a console subprocess doesn't flash a window.
#[cfg(target_os = "windows")]
pub fn new_std_command(program: impl AsRef<OsStr>) -> std::process::Command {
    use std::os::windows::process::CommandExt;

    let mut command = std::process::Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

/// A plain `std::process::Command` for `program`.
#[cfg(not(target_os = "windows"))]
pub fn new_std_command(program: impl AsRef<OsStr>) -> std::process::Command {
    std::process::Command::new(program)
}
