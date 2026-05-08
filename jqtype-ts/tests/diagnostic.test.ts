import { describe, expect, it } from "vitest";
import { Diagnostic, SourceSpan } from "../src/diagnostic.js";

describe("Diagnostic", () => {
  it("warning constructor", () => {
    const d = Diagnostic.warning("hi", SourceSpan.new(0, 3));
    expect(d).toEqual({
      severity: "Warning",
      message: "hi",
      span: { start: 0, end: 3 },
      source_name: null,
    });
  });

  it("error constructor", () => {
    const d = Diagnostic.error("bad", null);
    expect(d.severity).toBe("Error");
    expect(d.span).toBe(null);
  });

  it("withSourceName", () => {
    const d = Diagnostic.warning("hi", null);
    const named = Diagnostic.withSourceName(d, "filter.jq");
    expect(named.source_name).toBe("filter.jq");
    expect(d.source_name).toBe(null);
  });
});
