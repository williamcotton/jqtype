import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { JType } from "../src/types.js";
import {
  jsonSchemaToType,
  sampleToType,
  typeToJsonSchema,
  valueFitsType,
} from "../src/schema.js";

const here = dirname(fileURLToPath(import.meta.url));
const goldenDir = join(here, "../../jqtype-rs/tests/golden");

describe("jsonSchemaToType", () => {
  it("imports a basic object schema", () => {
    const schema = {
      type: "object",
      properties: {
        id: { type: "number" },
        name: { type: "string" },
      },
      required: ["id"],
      additionalProperties: false,
    };
    expect(JType.toCompactString(jsonSchemaToType(schema))).toBe(
      "object{id: number, name?: string}",
    );
  });

  it("imports the golden users schema", () => {
    const schema = JSON.parse(
      readFileSync(join(goldenDir, "users.schema.json"), "utf8"),
    );
    const ty = jsonSchemaToType(schema);
    expect(JType.toCompactString(ty)).toBe(
      "object{items: array<object{id: number, name: string}>}",
    );
  });

  it("handles enum, anyOf, allOf", () => {
    expect(JType.toCompactString(jsonSchemaToType({ enum: [1, 2, 3] }))).toBe(
      "1 | 2 | 3",
    );
    expect(
      JType.toCompactString(
        jsonSchemaToType({ anyOf: [{ type: "string" }, { type: "number" }] }),
      ),
    ).toBe("number | string");
    expect(
      JType.toCompactString(
        jsonSchemaToType({
          allOf: [
            {
              type: "object",
              properties: { a: { type: "number" } },
              required: ["a"],
              additionalProperties: false,
            },
            {
              type: "object",
              properties: { b: { type: "string" } },
              required: ["b"],
              additionalProperties: false,
            },
          ],
        }),
      ),
    ).toBe("object{a: number, b: string}");
  });

  it("integer and number both map to JType.number", () => {
    expect(jsonSchemaToType({ type: "integer" })).toEqual(JType.number());
    expect(jsonSchemaToType({ type: "number" })).toEqual(JType.number());
  });

  it("type as array yields a union", () => {
    expect(
      JType.toCompactString(jsonSchemaToType({ type: ["string", "null"] })),
    ).toBe("null | string");
  });

  it("missing additionalProperties defaults to open", () => {
    const ty = jsonSchemaToType({ type: "object", properties: {} });
    expect(JType.toCompactString(ty)).toBe("object{...}");
  });
});

describe("sampleToType", () => {
  it("infers shape from a sample", () => {
    const sample = { items: [{ name: "Ada" }, { name: "Grace" }] };
    expect(JType.toCompactString(sampleToType(sample))).toBe(
      'object{items: array<object{name: "Ada"} | object{name: "Grace"}>}',
    );
  });

  it("matches the golden users sample", () => {
    const sample = JSON.parse(
      readFileSync(join(goldenDir, "users.sample.json"), "utf8"),
    );
    const ty = sampleToType(sample);
    // User schema fits the sample-derived type.
    expect(valueFitsType(sample, ty)).toBe(true);
  });
});

describe("valueFitsType", () => {
  it("primitives and literals", () => {
    expect(valueFitsType(null, "Null")).toBe(true);
    expect(valueFitsType(false, "Null")).toBe(false);

    expect(valueFitsType(true, JType.bool())).toBe(true);
    expect(valueFitsType(true, JType.boolLit(true))).toBe(true);
    expect(valueFitsType(false, JType.boolLit(true))).toBe(false);

    expect(valueFitsType(42, JType.number())).toBe(true);
    expect(valueFitsType(42, JType.numberLit("42"))).toBe(true);
    expect(valueFitsType(42.0, JType.numberLit("42"))).toBe(true);
    expect(valueFitsType(43, JType.numberLit("42"))).toBe(false);

    expect(valueFitsType("hi", JType.string())).toBe(true);
    expect(valueFitsType("hi", JType.stringLit("hi"))).toBe(true);
  });

  it("arrays / objects / unions", () => {
    const arr = JType.array(JType.string());
    expect(valueFitsType(["a", "b"], arr)).toBe(true);
    expect(valueFitsType(["a", 1], arr)).toBe(false);

    const closed = JType.closedObject({
      id: JType.property(JType.number(), true),
      name: JType.property(JType.string(), false),
    });
    expect(valueFitsType({ id: 1, name: "Ada" }, closed)).toBe(true);
    expect(valueFitsType({ id: 1 }, closed)).toBe(true);
    expect(valueFitsType({ id: 1, extra: true }, closed)).toBe(false);
    expect(valueFitsType({ name: "Ada" }, closed)).toBe(false);

    const open = JType.openObject({
      id: JType.property(JType.number(), true),
      name: JType.property(JType.string(), false),
    });
    expect(valueFitsType({ id: 1, extra: true }, open)).toBe(true);

    const union = JType.union([JType.string(), "Null"]);
    expect(valueFitsType("x", union)).toBe(true);
    expect(valueFitsType(null, union)).toBe(true);
    expect(valueFitsType(1, union)).toBe(false);
  });

  it("Unknown accepts everything, Never accepts nothing", () => {
    expect(valueFitsType(null, "Unknown")).toBe(true);
    expect(valueFitsType({ any: [1, 2] }, "Unknown")).toBe(true);
    expect(valueFitsType(null, "Never")).toBe(false);
  });
});

describe("typeToJsonSchema round trip", () => {
  it("number / string / bool / null", () => {
    expect(typeToJsonSchema(JType.number())).toEqual({ type: "number" });
    expect(typeToJsonSchema(JType.string())).toEqual({ type: "string" });
    expect(typeToJsonSchema(JType.bool())).toEqual({ type: "boolean" });
    expect(typeToJsonSchema("Null")).toEqual({ type: "null" });
    expect(typeToJsonSchema("Never")).toEqual(false);
    expect(typeToJsonSchema("Unknown")).toEqual({});
  });

  it("number literal as JSON number", () => {
    expect(typeToJsonSchema(JType.numberLit("42"))).toEqual({ const: 42 });
  });

  it("object schema", () => {
    const ty = JType.closedObject({
      id: JType.property(JType.number(), true),
      name: JType.property(JType.string(), false),
    });
    expect(typeToJsonSchema(ty)).toEqual({
      type: "object",
      properties: {
        id: { type: "number" },
        name: { type: "string" },
      },
      required: ["id"],
      additionalProperties: false,
    });
  });
});
