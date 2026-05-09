import { describe, expect, it } from "vitest";
import { JType } from "../src/types.js";
import { StreamType } from "../src/stream.js";
import {
  AnalyzeOptions,
  InputShape,
  JQTYPE_CAPABILITIES,
  JqTypeChecker,
  analyzeFilter,
  type AnalyzeReport,
} from "../src/analyze.js";
import { jsonSchemaToType } from "../src/schema.js";

function check(filter: string, input: ReturnType<typeof JType.bool>): AnalyzeReport {
  return new JqTypeChecker().analyzeFilter(
    filter,
    InputShape.fromType(input),
    AnalyzeOptions.default(),
  );
}

describe("analyzer — small cases", () => {
  it("identity returns the input", () => {
    const r = check(".", JType.string());
    expect(StreamType.toCompactString(r.output)).toBe("string");
  });

  it("field projection on closed object", () => {
    const r = check(
      ".name",
      JType.closedObject({ name: JType.property(JType.string(), true) }),
    );
    expect(StreamType.toCompactString(r.output)).toBe("string");
  });

  it("array collection over iterated field", () => {
    const input = jsonSchemaToType({
      type: "object",
      properties: {
        items: {
          type: "array",
          items: {
            type: "object",
            properties: { name: { type: "string" } },
            required: ["name"],
            additionalProperties: false,
          },
        },
      },
      required: ["items"],
      additionalProperties: false,
    });
    const r = check("[.items[].name]", input);
    expect(StreamType.toCompactString(r.output)).toBe("array<string>");
  });

  it("object constructor with shorthand and explicit pair", () => {
    const input = jsonSchemaToType({
      type: "object",
      properties: {
        id: { type: "number" },
        user: {
          type: "object",
          properties: { name: { type: "string" } },
          required: ["name"],
          additionalProperties: false,
        },
      },
      required: ["id", "user"],
      additionalProperties: false,
    });
    const r = check("{ id, name: .user.name }", input);
    expect(StreamType.toCompactString(r.output)).toBe(
      "object{id: number, name: string}",
    );
  });

  it("select refines a discriminated union", () => {
    const input = jsonSchemaToType({
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
    });
    const r = check('select(.type == "user") | .name', input);
    expect(StreamType.toCompactString(r.output)).toBe(
      "Stream<string, ZeroOrOne>",
    );
  });

  it("select refines non-null field", () => {
    const input = jsonSchemaToType({
      type: "object",
      properties: { foo: { type: ["string", "null"] } },
      additionalProperties: false,
    });
    const r = check("select(.foo != null) | .foo", input);
    expect(StreamType.toCompactString(r.output)).toBe(
      "Stream<string, ZeroOrOne>",
    );
  });

  it("if refines non-null field", () => {
    const input = jsonSchemaToType({
      type: "object",
      properties: { foo: { type: ["string", "null"] } },
      additionalProperties: false,
    });
    const r = check('if .foo != null then .foo else "missing" end', input);
    expect(StreamType.toCompactString(r.output)).toBe('"missing" | string');
  });

  it("select refines via has", () => {
    const input = jsonSchemaToType({
      type: "object",
      properties: { foo: { type: "string" } },
      additionalProperties: false,
    });
    const r = check('select(has("foo")) | .foo', input);
    expect(StreamType.toCompactString(r.output)).toBe(
      "Stream<string, ZeroOrOne>",
    );
  });

  it("type predicate refines unknown", () => {
    const r = check('if type == "array" then [.[]] else null end', "Unknown");
    expect(StreamType.toCompactString(r.output)).toBe(
      "array<unknown> | null",
    );
  });

  it("builtins have useful signatures", () => {
    const input = jsonSchemaToType({
      type: "object",
      properties: {
        id: { type: "number" },
        name: { type: "string" },
      },
      required: ["id", "name"],
      additionalProperties: false,
    });

    expect(StreamType.toCompactString(check("keys", input).output)).toBe(
      'array<"id" | "name">',
    );
    expect(StreamType.toCompactString(check('has("name")', input).output)).toBe(
      "true",
    );
    expect(
      StreamType.toCompactString(
        check("values", JType.union(["Null", JType.string()])).output,
      ),
    ).toBe("Stream<string, ZeroOrOne>");
    expect(
      StreamType.toCompactString(
        check("strings", JType.union(["Null", JType.string()])).output,
      ),
    ).toBe("Stream<string, ZeroOrOne>");
    expect(
      StreamType.toCompactString(
        check("map(.name)", JType.array(input)).output,
      ),
    ).toBe("array<string>");
    expect(StreamType.toCompactString(check("length", input).output)).toBe(
      "number",
    );
  });

  it("variable binding preserves original dot", () => {
    const input = jsonSchemaToType({
      type: "object",
      properties: {
        foo: { type: "string" },
        bar: { type: "number" },
      },
      required: ["foo", "bar"],
      additionalProperties: false,
    });

    const r = check(".foo as $x | {x: $x, dot: .bar}", input);
    expect(r.unsupported_features).toHaveLength(0);
    expect(StreamType.toCompactString(r.output)).toBe(
      "object{dot: number, x: string}",
    );
  });

  it("conversions and plus support DSL shapes", () => {
    const input = jsonSchemaToType({
      type: "object",
      properties: {
        params: {
          type: "object",
          properties: { id: { type: "string" } },
          required: ["id"],
          additionalProperties: false,
        },
      },
      required: ["params"],
      additionalProperties: false,
    });

    const r = check(
      '{ id: (.params.id | tonumber), label: ("Team " + (.params.id | tostring)) }',
      input,
    );
    expect(r.unsupported_features).toHaveLength(0);
    expect(StreamType.toCompactString(r.output)).toBe(
      "object{id: number, label: string}",
    );
  });

  it("assignment updates identity-root paths", () => {
    const input = jsonSchemaToType({
      type: "object",
      properties: {
        method: { type: "string" },
      },
      required: ["method"],
      additionalProperties: false,
    });

    const r = check(".graphqlParams = { id: 1 }", input);
    expect(r.unsupported_features).toHaveLength(0);
    expect(StreamType.toCompactString(r.output)).toBe(
      "object{graphqlParams: object{id: 1}, method: string}",
    );
  });

  it("collection builtins cover DSL transforms", () => {
    const add = check("[10, 20, 30] | add", "Unknown");
    expect(add.unsupported_features).toHaveLength(0);
    expect(StreamType.toCompactString(add.output)).toBe("null | number");

    const input = jsonSchemaToType({
      type: "object",
      properties: {
        keys: {
          type: "array",
          items: { type: "number" },
        },
      },
      required: ["keys"],
      additionalProperties: false,
    });
    const joined = check('.keys | map(tostring) | join(",")', input);
    expect(joined.unsupported_features).toHaveLength(0);
    expect(StreamType.toCompactString(joined.output)).toBe("string");
  });

  it("slices and interpolation are analyzed", () => {
    const input = jsonSchemaToType({
      type: "object",
      properties: {
        body: { type: "string" },
        city: { type: "string" },
      },
      required: ["body", "city"],
      additionalProperties: false,
    });

    const slice = check('.body | .[0:50] + "..."', input);
    expect(slice.unsupported_features).toHaveLength(0);
    expect(StreamType.toCompactString(slice.output)).toBe("string");

    const interpolation = check('"Weather for \\(.city)"', input);
    expect(interpolation.unsupported_features).toHaveLength(0);
    expect(StreamType.toCompactString(interpolation.output)).toBe("string");
  });

  it("reduce dynamic update groups rows", () => {
    const input = jsonSchemaToType({
      type: "object",
      properties: {
        data: {
          type: "object",
          properties: {
            rows: {
              type: "array",
              items: {
                type: "object",
                properties: {
                  team_id: { type: ["string", "number"] },
                  name: { type: "string" },
                },
                required: ["team_id"],
                additionalProperties: true,
              },
            },
          },
          required: ["rows"],
          additionalProperties: false,
        },
      },
      required: ["data"],
      additionalProperties: false,
    });

    const r = check(
      "reduce .data.rows[] as $row ({}; .[$row.team_id | tostring] += [$row])",
      input,
    );
    expect(r.unsupported_features).toHaveLength(0);
    const compact = StreamType.toCompactString(r.output);
    expect(compact).toContain("object{}");
    expect(compact).toContain("...: array<object");
    expect(compact).toContain("team_id: number | string");
  });

  it("top-level sync API supports partial options and external variables", () => {
    const report = analyzeFilter(
      "{ user: $user.name, fallback: ($missing // \"none\") }",
      InputShape.unknown(),
      {
        externalVars: {
          user: JType.closedObject({
            name: JType.property(JType.stringLit("Ada"), true),
          }),
          missing: "Null",
        },
      },
    );

    expect(report.unsupported_features).toHaveLength(0);
    expect(StreamType.toCompactString(report.output)).toBe(
      'object{fallback: "none", user: "Ada"}',
    );
    expect(JSON.parse(JSON.stringify(report))).toEqual(report);
  });

  it("capability matrix is exported as plain data", () => {
    expect(JQTYPE_CAPABILITIES.some((cap) => cap.feature === "map")).toBe(true);
    expect(JSON.parse(JSON.stringify(JQTYPE_CAPABILITIES))).toEqual(
      JQTYPE_CAPABILITIES,
    );
  });

  it("diagnostics include jq-source spans where available", () => {
    const parseReport = analyzeFilter(".foo |");
    expect(parseReport.diagnostics[0]?.span).toEqual({ start: 6, end: 6 });

    const unsupported = analyzeFilter("group_by(.name)");
    expect(unsupported.diagnostics[0]?.span).toEqual({ start: 0, end: 8 });
    expect(unsupported.unsupported_features[0]?.span).toEqual({
      start: 0,
      end: 8,
    });

    const unbound = analyzeFilter("$context.foo");
    expect(unbound.diagnostics[0]?.span).toEqual({ start: 0, end: 8 });
  });

  it("unsupported builtin produces a warning", () => {
    const r = check("group_by(.name)", JType.array("Unknown"));
    expect(StreamType.toCompactString(r.output)).toBe("unknown");
    expect(r.unsupported_features.length).toBe(1);
    expect(r.diagnostics[0]?.message).toMatch(
      /unsupported builtin or call `group_by`/,
    );
  });

  it("comma joins streams", () => {
    const r = check(".a, .b", JType.closedObject({
      a: JType.property(JType.number(), true),
      b: JType.property(JType.string(), true),
    }));
    expect(StreamType.toCompactString(r.output)).toBe(
      "Stream<number | string, OneOrMore>",
    );
  });

  it("empty produces zero", () => {
    const r = check("empty", JType.string());
    expect(r.output.card).toBe("Zero");
  });

  it("Stream<…, ZeroOrMore> for iterating", () => {
    const r = check(".[]", JType.array(JType.number()));
    expect(StreamType.toCompactString(r.output)).toBe(
      "Stream<number, ZeroOrMore>",
    );
  });

  it("optional field access via ?", () => {
    const r = check(".foo?", JType.array(JType.number()));
    // .foo on an array: optional -> zero
    expect(r.output.card).toBe("Zero");
  });

  it("doc-test example", () => {
    const schema = jsonSchemaToType({
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
    });
    const r = check(".items[] | { id, name }", schema);
    expect(StreamType.toCompactString(r.output)).toBe(
      "Stream<object{id: number, name: string}, ZeroOrMore>",
    );
  });
});
