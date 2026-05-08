import { describe, expect, it } from "vitest";
import { JType } from "../src/types.js";

describe("JType.toCompactString", () => {
  it("formats scalars", () => {
    expect(JType.toCompactString("Never")).toBe("empty");
    expect(JType.toCompactString("Unknown")).toBe("unknown");
    expect(JType.toCompactString("Null")).toBe("null");
    expect(JType.toCompactString(JType.bool())).toBe("boolean");
    expect(JType.toCompactString(JType.boolLit(true))).toBe("true");
    expect(JType.toCompactString(JType.boolLit(false))).toBe("false");
    expect(JType.toCompactString(JType.number())).toBe("number");
    expect(JType.toCompactString(JType.numberLit(42))).toBe("42");
    expect(JType.toCompactString(JType.string())).toBe("string");
    expect(JType.toCompactString(JType.stringLit("hi"))).toBe('"hi"');
  });

  it("formats arrays and objects", () => {
    expect(JType.toCompactString(JType.array(JType.number()))).toBe(
      "array<number>",
    );

    const obj = JType.closedObject({
      id: JType.property(JType.number(), true),
      name: JType.property(JType.string(), false),
    });
    expect(JType.toCompactString(obj)).toBe(
      "object{id: number, name?: string}",
    );

    const open = JType.openObject({
      id: JType.property(JType.number(), true),
    });
    expect(JType.toCompactString(open)).toBe("object{id: number, ...}");

    const openTyped = JType.object(
      { id: JType.property(JType.number(), true) },
      JType.string(),
    );
    expect(JType.toCompactString(openTyped)).toBe(
      "object{id: number, ...: string}",
    );
  });

  it("sorts object properties alphabetically", () => {
    const obj = JType.closedObject({
      z: JType.property(JType.number(), true),
      a: JType.property(JType.string(), true),
    });
    expect(JType.toCompactString(obj)).toBe("object{a: string, z: number}");
  });
});

describe("JType.union", () => {
  it("collapses to single member", () => {
    expect(JType.union([JType.string()])).toEqual(JType.string());
  });

  it("returns Never for empty", () => {
    expect(JType.union([])).toBe("Never");
    expect(JType.union(["Never", "Never"])).toBe("Never");
  });

  it("absorbs Unknown", () => {
    expect(JType.union(["Unknown", JType.string()])).toBe("Unknown");
  });

  it("flattens nested unions", () => {
    const a = JType.union([JType.number(), JType.string()]);
    const b = JType.union([a, JType.bool()]);
    expect(b).toEqual({
      Union: [JType.bool(), JType.number(), JType.string()],
    });
  });

  it("dedupes by compact string", () => {
    expect(JType.union([JType.number(), JType.number()])).toEqual(
      JType.number(),
    );
  });
});

describe("JType.isTruthyLiteral", () => {
  it("static truthiness", () => {
    expect(JType.isTruthyLiteral("Null")).toBe(false);
    expect(JType.isTruthyLiteral(JType.boolLit(false))).toBe(false);
    expect(JType.isTruthyLiteral(JType.boolLit(true))).toBe(true);
    expect(JType.isTruthyLiteral(JType.number())).toBe(true);
    expect(JType.isTruthyLiteral(JType.string())).toBe(true);
  });

  it("returns null for indeterminate", () => {
    expect(JType.isTruthyLiteral("Unknown")).toBe(null);
    expect(JType.isTruthyLiteral(JType.bool())).toBe(null);
    expect(
      JType.isTruthyLiteral(JType.union([JType.string(), "Null"])),
    ).toBe(null);
  });
});

describe("JType.typeNames", () => {
  it("returns concrete type names", () => {
    expect(JType.typeNames("Null")).toEqual(["null"]);
    expect(JType.typeNames(JType.bool())).toEqual(["boolean"]);
    expect(JType.typeNames(JType.array(JType.number()))).toEqual(["array"]);
  });

  it("Unknown enumerates all types in declaration order", () => {
    expect(JType.typeNames("Unknown")).toEqual([
      "null",
      "boolean",
      "number",
      "string",
      "array",
      "object",
    ]);
  });

  it("Union flattens and sorts", () => {
    expect(
      JType.typeNames(JType.union([JType.number(), JType.string()])),
    ).toEqual(["number", "string"]);
  });
});

describe("JType.withoutNull", () => {
  it("removes null from a union", () => {
    expect(
      JType.withoutNull(JType.union([JType.string(), "Null"])),
    ).toEqual(JType.string());
  });

  it("removes null literal directly", () => {
    expect(JType.withoutNull("Null")).toBe("Never");
  });

  it("leaves non-null types alone", () => {
    expect(JType.withoutNull(JType.number())).toEqual(JType.number());
  });
});
