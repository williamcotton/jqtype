import { describe, expect, it } from "vitest";
import { parseFilter } from "../src/parser.js";

describe("parseFilter", () => {
  it("parses identity", () => {
    const r = parseFilter(".");
    expect(r.ok).toBe(true);
    if (r.ok) expect(r.ast.expr).toEqual({ type: "identity" });
  });

  it("parses field access", () => {
    const r = parseFilter(".foo");
    expect(r.ok).toBe(true);
    if (r.ok)
      expect(r.ast.expr).toEqual({
        type: "index",
        expr: { type: "identity" },
        index: "foo",
      });
  });

  it("parses pipe + iterator + object construction", () => {
    const r = parseFilter(".items[] | { id, name }");
    expect(r.ok).toBe(true);
    if (r.ok)
      expect(r.ast.expr).toEqual({
        type: "binary",
        operator: "|",
        left: {
          type: "iterator",
          expr: {
            type: "index",
            expr: { type: "identity" },
            index: "items",
          },
        },
        right: {
          type: "object",
          entries: [{ key: "id" }, { key: "name" }],
        },
      });
  });

  it("returns a failure for invalid input", () => {
    const r = parseFilter(".[");
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.message).toBeTruthy();
  });
});
