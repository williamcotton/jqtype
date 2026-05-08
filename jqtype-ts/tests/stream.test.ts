import { describe, expect, it } from "vitest";
import { JType } from "../src/types.js";
import { Cardinality, StreamType } from "../src/stream.js";

describe("Cardinality algebra", () => {
  it("join", () => {
    expect(Cardinality.join("Zero", "One")).toBe("One");
    expect(Cardinality.join("One", "Zero")).toBe("One");
    expect(Cardinality.join("One", "One")).toBe("OneOrMore");
    expect(Cardinality.join("ZeroOrOne", "ZeroOrOne")).toBe("ZeroOrMore");
    expect(Cardinality.join("OneOrMore", "ZeroOrOne")).toBe("ZeroOrMore");
  });

  it("alternative", () => {
    expect(Cardinality.alternative("Zero", "Zero")).toBe("Zero");
    expect(Cardinality.alternative("Zero", "One")).toBe("ZeroOrOne");
    expect(Cardinality.alternative("One", "One")).toBe("One");
    expect(Cardinality.alternative("One", "ZeroOrOne")).toBe("ZeroOrOne");
    expect(Cardinality.alternative("OneOrMore", "OneOrMore")).toBe(
      "OneOrMore",
    );
  });

  it("compose", () => {
    expect(Cardinality.compose("Zero", "One")).toBe("Zero");
    expect(Cardinality.compose("One", "ZeroOrMore")).toBe("ZeroOrMore");
    expect(Cardinality.compose("ZeroOrOne", "ZeroOrOne")).toBe("ZeroOrOne");
    expect(Cardinality.compose("ZeroOrOne", "ZeroOrMore")).toBe("ZeroOrMore");
    expect(Cardinality.compose("OneOrMore", "OneOrMore")).toBe("OneOrMore");
  });

  it("fitsCount", () => {
    expect(Cardinality.fitsCount("Zero", 0)).toBe(true);
    expect(Cardinality.fitsCount("Zero", 1)).toBe(false);
    expect(Cardinality.fitsCount("One", 1)).toBe(true);
    expect(Cardinality.fitsCount("One", 0)).toBe(false);
    expect(Cardinality.fitsCount("ZeroOrOne", 0)).toBe(true);
    expect(Cardinality.fitsCount("ZeroOrOne", 1)).toBe(true);
    expect(Cardinality.fitsCount("ZeroOrOne", 2)).toBe(false);
    expect(Cardinality.fitsCount("OneOrMore", 0)).toBe(false);
    expect(Cardinality.fitsCount("OneOrMore", 5)).toBe(true);
    expect(Cardinality.fitsCount("ZeroOrMore", 0)).toBe(true);
  });

  it("asStr", () => {
    expect(Cardinality.asStr("Zero")).toBe("zero");
    expect(Cardinality.asStr("ZeroOrOne")).toBe("zero_or_one");
    expect(Cardinality.asStr("OneOrMore")).toBe("one_or_more");
    expect(Cardinality.asStr("ZeroOrMore")).toBe("zero_or_more");
  });
});

describe("StreamType", () => {
  it("Zero card forces item to Never", () => {
    const s = StreamType.new(JType.number(), "Zero");
    expect(s.item).toBe("Never");
  });

  it("toCompactString", () => {
    expect(StreamType.toCompactString(StreamType.one(JType.number()))).toBe(
      "number",
    );
    expect(
      StreamType.toCompactString(StreamType.zeroOrMore(JType.number())),
    ).toBe("Stream<number, ZeroOrMore>");
  });

  it("join unions items and joins cardinalities", () => {
    const a = StreamType.one(JType.number());
    const b = StreamType.one(JType.string());
    const joined = StreamType.join(a, b);
    expect(joined.card).toBe("OneOrMore");
    expect(joined.item).toEqual(
      JType.union([JType.number(), JType.string()]),
    );
  });

  it("fitsOutputs checks card and item predicate", () => {
    const s = StreamType.zeroOrMore(JType.number());
    expect(
      StreamType.fitsOutputs(s, [1, 2], (v) => typeof v === "number"),
    ).toBe(true);
    expect(
      StreamType.fitsOutputs(s, [1, "hi"], (v) => typeof v === "number"),
    ).toBe(false);

    const oneOnly = StreamType.one(JType.number());
    expect(
      StreamType.fitsOutputs(oneOnly, [], (v) => typeof v === "number"),
    ).toBe(false);
    expect(
      StreamType.fitsOutputs(oneOnly, [1, 2], (v) => typeof v === "number"),
    ).toBe(false);
  });
});
