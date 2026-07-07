//! A `smol::process::Child` wrapper that spawns children as their own
//! session/process group (Unix) so an entire subprocess tree can be
//! signalled at once, and kills via `killpg` (Unix).
//!
//! Kills its process group on `Drop`: `smol::process::Child` does not kill
//! on drop, so without this a dropped handle would orphan the agent
//! subprocess tree for the host's lifetime. Explicit
//! [`Child::kill`]/`shutdown` paths still work — `Drop` is the backstop that
//! guarantees no orphan when a session/handle is simply dropped (e.g. a chat
//! tab closed).

use anyhow::{Context as _, Result};
use std::process::Stdio;

/// A wrapper around `smol::process::Child` that starts each subprocess in its
/// own session (Unix) so it — and its descendants — can be killed as a group.
///
/// `process` is `Option` only so [`Child::into_inner`] can move the inner
/// child out without tripping the `Drop` kill; it is `Some` for the whole
/// normal lifetime.
pub struct Child {
    process: Option<smol::process::Child>,
}

impl std::ops::Deref for Child {
    type Target = smol::process::Child;

    fn deref(&self) -> &Self::Target {
        self.process.as_ref().expect("Child used after into_inner")
    }
}

impl std::ops::DerefMut for Child {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.process.as_mut().expect("Child used after into_inner")
    }
}

impl Child {
    #[cfg(not(windows))]
    pub fn spawn(
        mut command: std::process::Command,
        stdin: Stdio,
        stdout: Stdio,
        stderr: Stdio,
    ) -> Result<Self> {
        super::set_pre_exec_to_start_new_session(&mut command);
        let mut command = smol::process::Command::from(command);
        let process = command
            .stdin(stdin)
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .with_context(|| {
                format!(
                    "failed to spawn command {}",
                    super::redact::redact_command(&format!("{command:?}"))
                )
            })?;
        Ok(Self {
            process: Some(process),
        })
    }

    #[cfg(windows)]
    pub fn spawn(
        command: std::process::Command,
        stdin: Stdio,
        stdout: Stdio,
        stderr: Stdio,
    ) -> Result<Self> {
        // Windows: descendants are not yet killed as a group (would require
        // a job object with the child process handle added to it, mirroring
        // the Unix session-group behavior). See
        // https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects
        let mut command = smol::process::Command::from(command);
        let process = command
            .stdin(stdin)
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .with_context(|| {
                format!(
                    "failed to spawn command {}",
                    super::redact::redact_command(&format!("{command:?}"))
                )
            })?;
        Ok(Self {
            process: Some(process),
        })
    }

    /// Consumes the wrapper, returning the inner child WITHOUT killing it
    /// (takes the child out so the `Drop` kill sees `None`).
    pub fn into_inner(mut self) -> smol::process::Child {
        self.process.take().expect("Child used after into_inner")
    }

    #[cfg(not(windows))]
    pub fn kill(&mut self) -> Result<()> {
        let Some(process) = self.process.as_mut() else {
            return Ok(());
        };
        let pid = process.id();
        // safety: killpg on the child's own process group (it is a session
        // leader, see `set_pre_exec_to_start_new_session`).
        unsafe {
            libc::killpg(pid as i32, libc::SIGKILL);
        }
        Ok(())
    }

    #[cfg(windows)]
    pub fn kill(&mut self) -> Result<()> {
        // No job object exists yet to terminate (see the note in `spawn`).
        if let Some(process) = self.process.as_mut() {
            process.kill()?;
        }
        Ok(())
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        // Guarantee the subprocess tree dies when its handle is dropped
        // (`smol::process::Child` has no kill-on-drop). No-op if already
        // taken via `into_inner`. Best-effort — a dead pgid just errors.
        if self.process.is_some() {
            let _ = self.kill();
        }
    }
}
