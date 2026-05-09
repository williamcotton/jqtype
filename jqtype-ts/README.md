# jqtype-ts

TypeScript port of [`jqtype`](../jqtype-rs) — static type analysis for [jq](https://stedolan.github.io/jq/) / [jaq](https://github.com/01mf02/jaq) filters.

The Rust crate at [`../jqtype-rs`](../jqtype-rs) is the reference implementation. `jqtype-ts` exists so the same analysis can be embedded in TypeScript-based tooling, in particular a language server for a host language that embeds jq/jaq filters.

## Status

Early scaffolding. The Rust analyzer is the source of truth; this package is being ported behavior-for-behavior against the same compatibility fixtures.

## Embedding

The package publishes both ESM and CommonJS entry points:

```js
import { analyzeFilter, InputShape, JType } from "jqtype";
// or: const { analyzeFilter, InputShape, JType } = require("jqtype");

const report = analyzeFilter(
  "$context.user.id | tostring",
  InputShape.unknown(),
  {
    externalVars: {
      context: JType.openObject({}),
    },
  },
);
```

`analyzeFilter(source, inputShape, options)` is synchronous and performs no filesystem, network, shell, or jq-binary calls. Reports are plain JSON-serializable objects containing the inferred `StreamType`, diagnostics, unsupported feature entries, and optional debug AST text.

Diagnostic spans use JavaScript string offsets into the jq source. In VS Code these are UTF-16 code-unit offsets.

## Capability Matrix

`JQTYPE_CAPABILITIES` is exported as plain data. Current WebPipe-useful coverage includes object and array constructors, field/index access, `map`, `select`, `if`, `//`, `length`, `type`, `tonumber`, `tostring`, object merge via `+`, external variables via `AnalyzeOptions.externalVars`, and partial support for identity-root updates such as `.field = ...`.

## Development

```sh
npm install
npm run check   # type-check
npm run build   # emit to dist/
npm test        # run vitest
```
