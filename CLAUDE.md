# Repository Guidelines

This repository hosts two implementations of the `jqtype` static analyzer for jq / jaq filters. They share design and test fixtures.

## Project Structure

- [`jqtype-rs/`](jqtype-rs/) — Rust reference implementation. Cargo workspace with `crates/jqtype-core` (library) and `crates/jqtype-cli` (CLI). This is the source of truth for analyzer behavior.
- [`jqtype-ts/`](jqtype-ts/) — TypeScript port for embedding in TS tooling (LSPs for languages that embed jq/jaq). Ports behavior-for-behavior from the Rust crate.
- `README.md` — repo-level overview pointing at both implementations.

Each subdirectory has its own `AGENTS.md` / `CLAUDE.md` with implementation-specific guidance. Read those before working inside a subdirectory.

## Cross-implementation rules

- **Rust is canonical.** When analyzer behavior differs between the two ports, the Rust crate wins. Update the TS port to match unless there is an explicit reason and a corresponding update on the Rust side.
- **Soundness over precision.** Both implementations prefer conservative widening (e.g., `Unknown`) over guessing. A type that excludes a possible runtime output is worse than one that is too broad.
- **Shared fixtures.** Compatibility fixtures (real jq executions of (filter, input) pairs) live under `jqtype-rs/`. The TS port should run against the same cases.
- **Public API parity.** The serializable surface — `JType`, `StreamType`, `Cardinality`, `Diagnostic`, `AnalyzeReport`, `InputShape` — should round-trip across both implementations via JSON.

## Working inside a sub-project

Stay inside the relevant subdirectory for build, test, and lint commands; do not invoke Rust tooling from the TS package or vice versa. Each implementation has its own dependency manifest, lockfile, and ignore rules.

- Rust: `cargo fmt --check`, `cargo clippy --all-targets`, `cargo test` (run from `jqtype-rs/`).
- TypeScript: `npm run check`, `npm run build`, `npm test` (run from `jqtype-ts/`).

## Repo-level compatibility harness

`compat.test.ts` at the repo root is the cross-implementation parity check. It runs each curated `(filter, schema, sample inputs)` case through:

1. The TS analyzer plus real `jq` (skipped when `jq` is not on `PATH`) — verifies every concrete `jq` output fits the inferred `StreamType`.
2. The TS analyzer's `--output json-schema`-shaped report against the Rust CLI's same output (skipped when `jqtype-rs/target/release/jqtype` has not been built) — verifies parity with the canonical implementation.

Run with `npm test` from the repo root. Build the Rust CLI first (`cd jqtype-rs && cargo build --release -p jqtype-cli`) so the second comparison runs; otherwise it self-skips. When changing analyzer behavior in either implementation, add a case here.

## Commits & PRs

Use clear imperative commit subjects. When a change touches analyzer semantics, prefer a single PR that updates both ports plus the shared fixtures, or open a tracking issue if only one side lands first. Pull requests should call out any divergence between the two ports.

## Agent-Specific Instructions

- Do not edit generated build artifacts (`jqtype-rs/target/`, `jqtype-ts/dist/`, `jqtype-ts/node_modules/`).
- Keep dependency additions minimal and justified in either implementation.
- When changing analyzer behavior, prefer conservative widening over false precision. If the Rust compat harness flags a soundness gap, fix the analyzer rather than weakening the assertion, then mirror the fix into the TS port.
