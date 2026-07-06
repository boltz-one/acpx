# 0003. Subprocess spawn/kill reuses `boltz-util::command`/`process`

- **Status:** accepted
- **Date:** 2026-07-05
- **Lane:** high-risk

## Context

acpx's `client-process.ts` implements spawn-readiness waiting, stdio
requiring, and a SIGTERM(1.5s)→SIGKILL(1s) grace-period kill sequence.
Direct inspection during planning found `crates/util/src/command.rs` (+
`command/darwin.rs`) and `crates/util/src/process.rs` already implement a
cross-platform `Command`/`Child`/`Stdio` wrapper on `smol::process`, with
macOS routed through a dedicated `posix_spawn`-based module (avoiding a known
`fork()`-related crash-reporter issue) and POSIX kill via `libc::killpg`
(whole process-group). Windows kill is a known-incomplete `Child::kill()`
with an upstream `TODO` noting job-object cleanup is not yet implemented.

## Decision

`crates/acp`'s `client/spawn.rs`, `client/shutdown.rs`, and
`terminal/{spawn,kill}.rs` depend on `boltz-util` and build directly on
`util::command::{Command, Stdio}` / `util::process::Child` rather than
writing new spawn/kill code. `util::process::Child::spawn` already starts
every command as its own POSIX session/process-group leader, so a single
`killpg` on its pid reaps its whole process-group tree.

`util::process::Child` only exposes SIGKILL (`killpg` + `SIGKILL`) — no
signal-specific kill. For the SIGTERM grace step, `client/shutdown.rs` and
`terminal/kill.rs` send `libc::killpg(pid, libc::SIGTERM)` directly
(documented inline as the one deliberate exception to "always go through
`util`"), then escalate to `util::process::Child::kill()` for SIGKILL.

## Alternatives Considered

- **Reimplement subprocess spawn/kill in `acp`.** Would duplicate real,
  already-hardened platform-specific code — the darwin `posix_spawn` path
  exists because of a real historical bug class. Reinventing it naively is
  both a DRY violation and a reliability regression risk.
- **Extend `util` with a signal-specific kill method** before using it. Would
  touch a near-universal workspace dependency for one caller's convenience;
  sending `libc::killpg(pid, SIGTERM)` directly at the two call sites that
  need it was judged simpler and lower-blast-radius.

## Consequences

- **Inherited Windows kill gap:** `util::process::Child::kill` has no
  job-object cleanup on Windows. Grandchildren of a killed agent or terminal
  process may survive on Windows. This is inherited risk, not introduced by
  this port — flagged here so it isn't silently lost, and documented in the
  crate's rustdoc.
- Terminal descendant-PID tracking (acpx's ~150-line `ps`-based walk) was not
  ported: because every terminal command is already its own process-group
  leader via `util::process::Child::spawn`, one `killpg` already reaps the
  whole tree without separate PID-tree tracking.
