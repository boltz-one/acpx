# 0002. Async substrate is `smol`; no tokio, no bespoke executor, no `boltz-scheduler`

- **Status:** accepted
- **Date:** 2026-07-05
- **Lane:** high-risk

## Context

This workspace has zero tokio dependencies anywhere (confirmed by direct
`grep` across all `crates/*/Cargo.toml` at planning time). `crates/acp` needs
an async runtime for subprocess I/O (spawning ACP agent subprocesses,
non-blocking stdio JSON-RPC) and its own internal task spawning. Initial
research recommended a bare `async-task` + `async-executor` pair as the
primary fit, with `smol` as a heavier-but-viable alternative, without
visibility into `crates/util`'s existing usage. Separately, `crates/scheduler`
(`boltz-scheduler`) defines a `Scheduler` trait used by GPUI's own
foreground/background task dispatch, built on `async-task::Runnable`.

## Decision

Use `smol` directly as `acp`'s async substrate (`smol::spawn`,
`smol::io::{BufReader, AsyncBufReadExt}`, `smol::Timer`, `smol::process`).
Do not build a bespoke `async-task`+`async-executor` pair, and do not couple
`acp`'s core engine to `boltz-scheduler::Scheduler`.

## Alternatives Considered

- **Bare `async-task` + `async-executor`** (research's top pick before
  `crates/util` was inspected). `smol` already ships this exact combination
  internally (it re-exports/builds on `async-task`, `async-io`,
  `async-executor`) plus a ready-made process module and stdio async I/O.
  `smol` is also *already* a workspace dependency (`crates/util`,
  `crates/gpui_linux`), unlike bare `async-executor`, which nothing in this
  codebase used yet. Reusing an existing dependency beats introducing a new
  one.
- **`boltz-scheduler::Scheduler`.** Its `SessionId` type identifies a GPUI
  *window* session — an unrelated concept that would collide/confuse with
  ACP's own "session" vocabulary throughout this crate — and the trait is
  designed for classifying UI-thread-adjacent foreground vs. background work
  inside a running GPUI `App`. Coupling `acp`'s engine to it would make the
  crate untestable without a full GPUI `TestAppContext`, contradicting the
  requirement that the embeddable engine run standalone (e.g. a plain
  `#[test]` spawning a real fake-agent subprocess, no GPUI window needed).
- **tokio.** Explicitly excluded — zero tokio anywhere in this workspace.

## Consequences

- `crates/util::command`/`process` (already smol-based) is the natural reuse
  target for subprocess spawn/kill — see
  [0003](./0003-reuse-util-command-for-subprocess.md).
- The GPUI app bridges `acp`'s smol-driven futures into its own UI update
  cycle at the app layer (e.g. `cx.background_executor().spawn(...)`
  wrapping an `acp` future, or a channel from smol's global executor back to
  GPUI's foreground executor) — this crate only guarantees its futures are
  `Send + 'static`, and does not prescribe the bridging strategy.
- `acp`'s test suite runs without any GPUI dependency at all, which is what
  let every phase ship real-subprocess integration tests as plain
  `#[test]`/`#[cfg(test)]` code.
