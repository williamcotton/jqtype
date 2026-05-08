import { describe, expect, it } from "vitest";
import { JType } from "../src/types.js";
import { StreamType } from "../src/stream.js";
import {
  AnalyzeOptions,
  InputShape,
  JqTypeChecker,
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
