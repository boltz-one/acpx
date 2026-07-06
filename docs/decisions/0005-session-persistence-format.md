# 0005. Session persistence format & versioning strategy

- **Status:** accepted
- **Date:** 2026-07-05
- **Lane:** high-risk

## Context

acpx's on-disk session format is a hand-validated JSON convention: a
`schema` string tag (`"acpx.session.v1"`), a `persisted-key-policy.ts`
runtime assertion enforcing snake_case keys (a lint-at-runtime safety net
against TS's default camelCase `JSON.stringify` output drifting into a
deliberately snake_case persisted format), and `parse.ts`'s 878 lines of
manual, field-by-field forward/backward-compat logic.

## Decision

Rust's `serde` gives most of these guarantees structurally, but they must be
*deliberately* engineered, not assumed:

- **Schema tag:** `SessionSchemaVersion` is a Rust enum (not a bare string),
  so an unrecognized future schema version fails to deserialize *explicitly*
  rather than silently coercing — a version-sniffing pre-pass (parse as
  `serde_json::Value` first, branch on `schema`, then deserialize into the
  matching versioned struct) replaces acpx's runtime string compare with a
  compile-time-checked enum match.
- **Forward-compat:** every persisted struct carries
  `#[serde(flatten)] extra: serde_json::Map<String, serde_json::Value>` to
  capture and round-trip fields the current Rust struct doesn't know about —
  the structural analog of acpx's "ignore unknown fields, don't drop them on
  next write."
- **Backward-compat:** every field acpx's `parse.ts` treats as
  optional-with-default gets `#[serde(default)]`.
- **Snake_case enforcement:** ordinary Rust struct fields are snake_case by
  construction, so the entire bug class `persisted-key-policy.ts` guards
  against doesn't exist here for plain fields. The policy isn't fully dead,
  though: acpx's internally-tagged enum variants (`User`/`Agent`/`Text`/
  `Mention`/etc.) are deliberately PascalCase tags inside an otherwise
  snake_case document — ported as internally-tagged Rust enums with explicit
  `#[serde(rename = "User")]` per variant, backed by a **debug-only**
  regression test (not a release-build runtime assertion, since the
  guarantee is now structural) against future accidental
  `#[serde(rename_all = "camelCase")]` additions.

## Alternatives Considered

- **Transliterate `parse.ts`'s 878 lines of manual default-filling
  line-by-line.** A straight DRY violation of what `serde`'s
  `#[serde(default)]`/`#[serde(flatten)]` already give the language — though
  `parse.ts`'s test cases remained essential input for getting the struct
  definitions' defaults right.
- **A bare `String` schema tag matching acpx exactly.** Would let an
  unrecognized future schema version silently attempt to deserialize into
  the current struct and fail with a generic serde error, instead of a clear
  "unsupported schema version" error.

## Consequences

- Session files are not byte-compatible with acpx's `~/.acpx/sessions/*.json`
  — this is a new format (`boltz-acpx.session.v1`), not a shared one; no
  migration path from existing acpx session files was implemented or
  requested.
- The default session-storage root is `dirs::state_dir()/boltz-acpx`, deliberately
  distinct from acpx's `.acpx` naming to avoid cross-tool confusion if both
  are installed on the same machine, and is exposed as a configurable field
  (`AcpFileSessionStoreOptions`-equivalent) rather than hardcoded.
- Adding a genuinely new schema version later means adding a new enum variant
  and a migration branch in the version-sniffing pre-pass — a compile-time-
  visible change, not a silent runtime one.
