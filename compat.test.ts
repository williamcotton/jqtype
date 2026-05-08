// Cross-implementation compatibility harness.
//
// Lives at repo root because it verifies parity between `jqtype-rs` (Rust
// reference) and `jqtype-ts` (TypeScript port). Each case is a (filter,
// schema, sample inputs) tuple. For every case we:
//
//   1. Run the TS analyzer and ensure the inferred StreamType is sound for
//      every concrete jq output (real `jq` shelled out via PATH).
//   2. Compare the TS analyzer's `json-schema`-shaped report against the
//      Rust CLI's `--output json-schema` output.
//
// The `jq` check is skipped when `jq` is not on PATH; the Rust comparison is
// skipped when `jqtype-rs/target/release/jqtype` has not been built.

import { describe, expect, it } from "vitest";
import { spawnSync } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  AnalyzeOptions,
  AnalyzeReport,
  InputShape,
  JqTypeChecker,
} from "./jqtype-ts/src/analyze.js";
import { Cardinality, StreamType } from "./jqtype-ts/src/stream.js";
import { valueFitsType } from "./jqtype-ts/src/schema.js";

interface Case {
  name: string;
  filter: string;
  inputSchema: unknown;
  inputs: unknown[];
}

const usersSchema = {
  type: "object",
  properties: {
    items: {
      type: "array",
      items: {
        type: "object",
        properties: {
          id: { type: "number" },
          name: { type: "string" },
        },
        required: ["id", "name"],
        additionalProperties: false,
      },
    },
  },
  required: ["items"],
  additionalProperties: false,
};

const usersSample = {
  items: [
    { id: 1, name: "Ada" },
    { id: 2, name: "Grace" },
  ],
};
const emptyUsers = { items: [] };

const discriminatedSchema = {
  type: "array",
  items: {
    anyOf: [
      {
        type: "object",
        properties: {
          type: { enum: ["user"] },
          name: { type: "string" },
        },
        required: ["type", "name"],
        additionalProperties: false,
      },
      {
        type: "object",
        properties: {
          type: { enum: ["org"] },
          org_name: { type: "string" },
        },
        required: ["type", "org_name"],
        additionalProperties: false,
      },
    ],
  },
};
const discriminatedSample = [
  { type: "user", name: "Ada" },
  { type: "org", org_name: "Anthropic" },
  { type: "user", name: "Grace" },
];

const activeSchema = {
  type: "object",
  properties: {
    active: { type: "boolean" },
    name: { type: "string" },
  },
  required: ["active", "name"],
  additionalProperties: false,
};

const nullableFooSchema = {
  type: "object",
  properties: { foo: { type: ["string", "null"] } },
  required: ["foo"],
  additionalProperties: false,
};

const cases: Case[] = [
  {
    name: "identity",
    filter: ".",
    inputSchema: usersSchema,
    inputs: [usersSample, emptyUsers],
  },
  {
    name: "field projection",
    filter: ".items",
    inputSchema: usersSchema,
    inputs: [usersSample, emptyUsers],
  },
  {
    name: "iterate items",
    filter: ".items[]",
    inputSchema: usersSchema,
    inputs: [usersSample, emptyUsers],
  },
  {
    name: "collect projection",
    filter: "[.items[].name]",
    inputSchema: usersSchema,
    inputs: [usersSample, emptyUsers],
  },
  {
    name: "object construction",
    filter: ".items[] | {id, name}",
    inputSchema: usersSchema,
    inputs: [usersSample, emptyUsers],
  },
  {
    name: "if then else over booleans",
    filter: "if .active then .name else null end",
    inputSchema: activeSchema,
    inputs: [
      { active: true, name: "Ada" },
      { active: false, name: "Grace" },
    ],
  },
  {
    name: "select on discriminated union",
    filter: '.[] | select(.type == "user") | .name',
    inputSchema: discriminatedSchema,
    inputs: [discriminatedSample, []],
  },
  {
    name: "comma combines streams",
    filter: ".items[0].id, .items[0].name",
    inputSchema: usersSchema,
    inputs: [usersSample],
  },
  {
    name: "map projection",
    filter: ".items | map(.id)",
    inputSchema: usersSchema,
    inputs: [usersSample, emptyUsers],
  },
  {
    name: "type builtin",
    filter: ".items[] | type",
    inputSchema: usersSchema,
    inputs: [usersSample],
  },
  {
    name: "length builtin",
    filter: ".items | length",
    inputSchema: usersSchema,
    inputs: [usersSample, emptyUsers],
  },
  {
    name: "keys on object",
    filter: ".items[0] | keys",
    inputSchema: usersSchema,
    inputs: [usersSample],
  },
  {
    name: "empty produces no outputs",
    filter: "empty",
    inputSchema: usersSchema,
    inputs: [usersSample],
  },
  {
    name: "select non-null then field",
    filter: "select(.foo != null) | .foo",
    inputSchema: nullableFooSchema,
    inputs: [{ foo: "x" }, { foo: null }],
  },
  {
    name: "string filter narrows union",
    filter: ".foo | strings",
    inputSchema: nullableFooSchema,
    inputs: [{ foo: "x" }, { foo: null }],
  },
  {
    name: "math op stays sound on string concat",
    filter: '.items[0].name + "!"',
    inputSchema: usersSchema,
    inputs: [usersSample],
  },
];

function which(cmd: string): string | null {
  // Strip node_modules/.bin from PATH so bundled shims (e.g. @jq-tools/jq's
  // `jq` shim) don't shadow the system binary we actually want for compat.
  const cleanPath = (process.env.PATH ?? "")
    .split(":")
    .filter((p) => !p.includes("node_modules/.bin"))
    .join(":");
  const result = spawnSync("which", [cmd], {
    encoding: "utf8",
    env: { ...process.env, PATH: cleanPath },
  });
  if (result.status !== 0) return null;
  return result.stdout.trim();
}

function commandExists(path: string): boolean {
  try {
    const r = spawnSync(path, ["--help"], { stdio: "ignore" });
    return r.status === 0 || r.status === 1 || r.status === 2;
  } catch {
    return false;
  }
}

const HERE = dirname(fileURLToPath(import.meta.url));
const JQ_PATH = which("jq");
const RUST_CLI = resolve(HERE, "jqtype-rs/target/release/jqtype");
const jqAvailable = JQ_PATH !== null;
const rustCliAvailable = commandExists(RUST_CLI);

function runJq(filter: string, input: unknown): unknown[] {
  if (JQ_PATH === null) throw new Error("jq not on PATH");
  const r = spawnSync(JQ_PATH, ["-c", filter], {
    input: JSON.stringify(input),
    encoding: "utf8",
  });
  if (r.status !== 0) {
    throw new Error(`jq exited ${r.status}: ${r.stderr}`);
  }
  return r.stdout
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line));
}

function runRustCliJsonSchema(filter: string, schema: unknown): unknown {
  const dir = mkdtempSync(join(tmpdir(), "jqtype-compat-"));
  const path = join(dir, "schema.json");
  writeFileSync(path, JSON.stringify(schema));
  const r = spawnSync(
    RUST_CLI,
    ["--input-schema", path, "--output", "json-schema", filter],
    { encoding: "utf8" },
  );
  if (r.status !== 0) {
    throw new Error(`rust cli exited ${r.status}: ${r.stderr}`);
  }
  return JSON.parse(r.stdout);
}

const checker = new JqTypeChecker();

describe("compat: jq output fits inferred stream type", () => {
  for (const c of cases) {
    it.skipIf(!jqAvailable)(c.name, () => {
      const report = checker.analyzeFilter(
        c.filter,
        InputShape.fromJsonSchema(c.inputSchema),
        AnalyzeOptions.default(),
      );

      expect(
        AnalyzeReport.hasErrors(report),
        `analysis errors: ${JSON.stringify(report.diagnostics)}`,
      ).toBe(false);

      const stream = report.output;
      for (const [index, input] of c.inputs.entries()) {
        const outputs = runJq(c.filter, input);
        expect(
          Cardinality.fitsCount(stream.card, outputs.length),
          `case "${c.name}" input #${index}: ${outputs.length} outputs do not fit ${stream.card} (${StreamType.toCompactString(stream)})`,
        ).toBe(true);
        for (const [outIdx, value] of outputs.entries()) {
          expect(
            valueFitsType(value, stream.item),
            `case "${c.name}" input #${index} output #${outIdx}: ${JSON.stringify(value)} does not fit ${StreamType.toCompactString(stream)}`,
          ).toBe(true);
        }
      }
    });
  }
});

describe("compat: TS json-schema output matches Rust CLI", () => {
  for (const c of cases) {
    it.skipIf(!rustCliAvailable)(c.name, () => {
      const tsReport = checker.analyzeFilter(
        c.filter,
        InputShape.fromJsonSchema(c.inputSchema),
        AnalyzeOptions.default(),
      );
      const tsJson = AnalyzeReport.toJsonSchemaValue(tsReport);
      const rustJson = runRustCliJsonSchema(c.filter, c.inputSchema);
      expect(tsJson).toEqual(rustJson);
    });
  }
});

describe("Cardinality.fitsCount matrix", () => {
  it("matches Rust", () => {
    expect(Cardinality.fitsCount("Zero", 0)).toBe(true);
    expect(Cardinality.fitsCount("Zero", 1)).toBe(false);
    expect(Cardinality.fitsCount("One", 1)).toBe(true);
    expect(Cardinality.fitsCount("One", 0)).toBe(false);
    expect(Cardinality.fitsCount("One", 2)).toBe(false);
    expect(Cardinality.fitsCount("ZeroOrOne", 0)).toBe(true);
    expect(Cardinality.fitsCount("ZeroOrOne", 1)).toBe(true);
    expect(Cardinality.fitsCount("ZeroOrOne", 2)).toBe(false);
    expect(Cardinality.fitsCount("OneOrMore", 0)).toBe(false);
    expect(Cardinality.fitsCount("OneOrMore", 1)).toBe(true);
    expect(Cardinality.fitsCount("OneOrMore", 7)).toBe(true);
    expect(Cardinality.fitsCount("ZeroOrMore", 0)).toBe(true);
    expect(Cardinality.fitsCount("ZeroOrMore", 99)).toBe(true);
  });
});
