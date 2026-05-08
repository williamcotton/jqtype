# jqtype-ts

TypeScript port of [`jqtype`](../jqtype-rs) — static type analysis for [jq](https://stedolan.github.io/jq/) / [jaq](https://github.com/01mf02/jaq) filters.

The Rust crate at [`../jqtype-rs`](../jqtype-rs) is the reference implementation. `jqtype-ts` exists so the same analysis can be embedded in TypeScript-based tooling, in particular a language server for a host language that embeds jq/jaq filters.

## Status

Early scaffolding. The Rust analyzer is the source of truth; this package is being ported behavior-for-behavior against the same compatibility fixtures.

## Development

```sh
npm install
npm run check   # type-check
npm run build   # emit to dist/
npm test        # run vitest
```
