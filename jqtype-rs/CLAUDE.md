# Repository Guidelines

## Project Structure & Module Organization

This is a Rust workspace with two crates:

- `crates/jqtype-core/`: importable library for jq static type analysis. The primary product surface.
- `crates/jqtype-cli/`: thin command-line wrapper around `jqtype-core`.
- `crates/jqtype-core/src/analyze.rs`: parser adapter and analyzer rules.
- `crates/jqtype-core/src/types.rs`: JSON shape model.
- `crates/jqtype-core/src/stream.rs`: stream cardinality model and `StreamType`.
- `crates/jqtype-core/src/schema.rs`: sample / JSON Schema conversion plus `value_fits_type`.
- `crates/jqtype-core/src/diagnostic.rs`: diagnostic and span types.
- `crates/jqtype-core/examples/`: embedding examples (`embedding.rs`, `host_app.rs`).
- `crates/jqtype-core/tests/compat.rs`: jq compatibility harness.
- `tests/golden/`: JSON fixtures used for CLI/manual checks.
- `README.md`: user-facing overview, CLI usage, and library embedding example.

Keep analysis logic in `jqtype-core`; the CLI should only handle args, file I/O, rendering, and exit codes.

## Build, Test, and Development Commands

- `cargo fmt --check`: verify Rust formatting.
- `cargo fmt`: apply standard formatting.
- `cargo check`: fast compile/type check for the workspace.
- `cargo clippy --all-targets`: lint the workspace including examples and tests.
- `cargo test`: run unit, doc, and integration tests (the compat harness skips itself when `jq` is not on `PATH`).
- `cargo build --release -p jqtype-cli`: build optimized CLI at `target/release/jqtype`.
- `cargo run -p jqtype-cli -- --input-schema tests/golden/users.schema.json '.items | map(.name)'`: run the CLI in dev mode.
- `cargo run -q -p jqtype-cli -- --input-schema tests/golden/users.schema.json --output json-schema '.items[] | {id, name}'`: run the CLI in dev mode and output the result as JSON Schema.
- `cargo run --example host_app -p jqtype-core`: run the embedding example.
- `target/release/jqtype --input-schema tests/golden/users.schema.json '.items | map(.name)'`: run the production binary directly.

## Coding Style & Naming Conventions

Use Rust 2024 edition and standard `rustfmt` formatting. Prefer small, focused modules and public API types that are serializable when they are part of embedding output. Use `snake_case` for functions/modules, `PascalCase` for types/enums, and concise enum variants such as `Unknown`, `Never`, and `ZeroOrMore`.

Do not expose `jaq-syn` AST types from the stable `jqtype-core` API unless necessary. Public types meant for embedding (`JType`, `StreamType`, `Cardinality`, `Diagnostic`, `AnalyzeReport`, `InputShape`) must implement `serde::Serialize` and `Deserialize`.

## Testing Guidelines

Tests use Rust's built-in test framework. Place unit tests near the module they exercise. Name tests by behavior, for example `select_refines_discriminated_union` or `unsupported_builtin_reports_warning`.

For analyzer changes, add focused tests for inferred compact output and diagnostics. When adding or changing a soundness-relevant rule (cardinality, builtin signature, refinement), also add a case to `crates/jqtype-core/tests/compat.rs` so real `jq` validates the inference. Run one CLI smoke test against `tests/golden/users.schema.json`.

## Commit & Pull Request Guidelines

The history currently contains only an initial commit, so no repository-specific convention is established. Use clear imperative commit subjects, for example `Add select predicate refinement`.

Pull requests should include a short summary, key behavior changes, test commands run, and any known precision/soundness tradeoffs. Include CLI examples when changing user-facing output.

## Agent-Specific Instructions

Avoid editing generated build artifacts under `target/`. Keep dependency additions minimal and justified. When changing analyzer behavior, prefer conservative widening over false precision — a type that excludes a possible runtime output is worse than one that is too broad. If the compat harness flags a soundness gap, fix the analyzer rather than weakening the assertion.
