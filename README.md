# jqtype

Static type analysis for [jq](https://stedolan.github.io/jq/) / [jaq](https://github.com/01mf02/jaq) filters.

`jqtype` infers the output JSON shape of a filter given an input shape. The central model is `Filter : InputType -> Stream<OutputType>` — every filter is treated as a function from one input value to a stream that may contain zero, one, or many output values, preserving cardinality information for filters like `.items[]`, `empty`, `select(...)`, and `,`.

## Implementations

This repo houses two implementations of the same analyzer, sharing test fixtures and design:

- [`jqtype-rs/`](jqtype-rs/) — the Rust reference implementation. Library (`jqtype-core`) plus CLI (`jqtype-cli`). This is the source of truth for analysis behavior.
- [`jqtype-ts/`](jqtype-ts/) — a TypeScript port intended for embedding in TS-based tooling, in particular a language server for a host language that embeds jq/jaq filters.

The Rust crate is the canonical implementation; the TS package is being ported behavior-for-behavior against the same compatibility harness.

## Quick start

Rust CLI:

```sh
cd jqtype-rs
cargo build --release -p jqtype-cli
target/release/jqtype --input-schema tests/golden/users.schema.json '.items[] | { id, name }'
# Stream<object{id: number, name: string}, ZeroOrMore>
```

TypeScript:

```sh
cd jqtype-ts
npm install
npm run build
```

See each directory's `README.md` for full usage and embedding examples.

## Compatibility harness

The repo root contains `compat.test.ts`, a parity test that exercises both implementations against the same curated `(filter, schema, sample input)` cases:

```sh
npm install                                       # root deps (vitest)
cd jqtype-rs && cargo build --release -p jqtype-cli && cd ..  # build canonical CLI
npm test                                          # run the harness
```

The harness runs every case through TS + real `jq` (asserts outputs fit the inferred `StreamType`) and through TS + the Rust CLI (asserts the json-schema-shaped report matches). Each layer self-skips if the corresponding binary isn't available.
