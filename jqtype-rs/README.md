# jqtype

`jqtype` is a Rust library and CLI that statically analyzes [jq](https://stedolan.github.io/jq/) filters and reports the possible output JSON shape for a supplied input shape.

The central model is `Filter : InputType -> Stream<OutputType>` — every filter is treated as a function from one input value to a stream that may contain zero, one, or many output values. That preserves cardinality information for filters like `.items[]`, `empty`, `select(...)`, and `,`.

## Workspace layout

- `crates/jqtype-core/` — the importable library. All analysis, parsing, and JSON-Schema conversion lives here. This is the primary product surface.
- `crates/jqtype-cli/` — a thin command-line wrapper around `jqtype-core`.

## CLI

```sh
cargo build --release -p jqtype-cli
target/release/jqtype --input-schema tests/golden/users.schema.json '.items[] | { id, name }'
```

```text
Stream<object{id: number, name: string}, ZeroOrMore>
```

Render JSON Schema instead:

```sh
target/release/jqtype \
  --input-schema tests/golden/users.schema.json \
  --output json-schema \
  '.items[] | { id, name }'
```

Other useful flags:

- `--sample <PATH>` — infer a precise type from a single JSON sample.
- `--strict` — surface possibly-invalid runtime operations as errors with a non-zero exit code.
- `--debug-ast` — print the parsed jaq AST and exit.

## Library use

`jqtype-core` is the API any host application should depend on. It exposes a stable, serializable `AnalyzeReport` and accepts input shapes as either an in-memory [`JType`], a JSON Schema, or a sample value.

Add it as a git dependency:

```toml
[dependencies]
jqtype-core = { git = "https://github.com/williamcotton/jqtype", package = "jqtype-core" }
serde_json = "1"
```

Or via a local path during development:

```toml
[dependencies]
jqtype-core = { path = "../jqtype/crates/jqtype-core" }
```

Then call the analyzer:

```rust
use jqtype_core::{AnalyzeOptions, InputShape, JqTypeChecker};
use serde_json::json;

fn main() {
    let schema = json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id":   { "type": "number" },
                        "name": { "type": "string" }
                    },
                    "required": ["id", "name"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["items"],
        "additionalProperties": false
    });

    let report = JqTypeChecker::new().analyze_filter(
        ".items[] | { id, name }",
        InputShape::from_json_schema(schema),
        AnalyzeOptions::default(),
    );

    println!("{}", report.output_type().to_compact_string());
    // -> Stream<object{id: number, name: string}, ZeroOrMore>
}
```

If your host already has a type model, build a [`JType`] directly and pass `InputShape::from_type(...)`. The full embedding example lives at [`crates/jqtype-core/examples/host_app.rs`](crates/jqtype-core/examples/host_app.rs):

```sh
cargo run --example host_app -p jqtype-core
```

`AnalyzeReport`, `JType`, `StreamType`, `Cardinality`, and `Diagnostic` all implement `serde::Serialize`/`Deserialize`, so reports can be cached, logged, or shipped over IPC.

## Verifying actual values fit a type

`value_fits_type(&serde_json::Value, &JType) -> bool` is the same predicate the compatibility harness uses. Hosts can use it to validate concrete data against a derived shape:

```rust
use jqtype_core::{value_fits_type, JType};
use serde_json::json;

assert!(value_fits_type(&json!("hi"), &JType::string()));
assert!(!value_fits_type(&json!(1),    &JType::string()));
```

## Soundness

The analyzer prefers conservative widening over false precision: ambiguous or unsupported operations widen to `Unknown` and emit a diagnostic rather than guess. A type that excludes possible runtime outputs is worse than one that is too broad.

The compatibility harness in `crates/jqtype-core/tests/compat.rs` runs real `jq` (when present in `PATH`) on a curated set of (filter, input) pairs and asserts that every concrete output fits the inferred `StreamType`.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets
cargo test
```

`cargo test` runs the unit tests, the doc-test on `lib.rs`, and the compatibility harness. The harness automatically skips itself when `jq` is not present in `PATH`.

See `implementation-plan.md` for the full design rationale and milestone tracker.
