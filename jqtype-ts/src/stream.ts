import { JType, type JType as JTypeT } from "./types.js";

export type Cardinality =
  | "Zero"
  | "One"
  | "ZeroOrOne"
  | "OneOrMore"
  | "ZeroOrMore";

export const Cardinality = {
  asStr(c: Cardinality): string {
    switch (c) {
      case "Zero": return "zero";
      case "One": return "one";
      case "ZeroOrOne": return "zero_or_one";
      case "OneOrMore": return "one_or_more";
      case "ZeroOrMore": return "zero_or_more";
    }
  },

  join(a: Cardinality, b: Cardinality): Cardinality {
    if (a === "Zero") return b;
    if (b === "Zero") return a;
    if (a === "One" && b === "One") return "OneOrMore";
    if (
      (a === "One" && b === "ZeroOrOne") ||
      (a === "ZeroOrOne" && b === "One") ||
      (a === "ZeroOrOne" && b === "ZeroOrOne")
    ) {
      return "ZeroOrMore";
    }
    return "ZeroOrMore";
  },

  alternative(a: Cardinality, b: Cardinality): Cardinality {
    if (a === "Zero" && b === "Zero") return "Zero";
    if (
      (a === "Zero" && b === "One") ||
      (a === "One" && b === "Zero") ||
      (a === "Zero" && b === "ZeroOrOne") ||
      (a === "ZeroOrOne" && b === "Zero")
    ) {
      return "ZeroOrOne";
    }
    if (
      (a === "Zero" && b === "OneOrMore") ||
      (a === "OneOrMore" && b === "Zero") ||
      (a === "Zero" && b === "ZeroOrMore") ||
      (a === "ZeroOrMore" && b === "Zero")
    ) {
      return "ZeroOrMore";
    }
    if (a === "One" && b === "One") return "One";
    if (
      (a === "One" && b === "ZeroOrOne") ||
      (a === "ZeroOrOne" && b === "One") ||
      (a === "ZeroOrOne" && b === "ZeroOrOne")
    ) {
      return "ZeroOrOne";
    }
    if (
      (a === "One" && b === "OneOrMore") ||
      (a === "OneOrMore" && b === "One") ||
      (a === "OneOrMore" && b === "OneOrMore")
    ) {
      return "OneOrMore";
    }
    return "ZeroOrMore";
  },

  fitsCount(c: Cardinality, count: number): boolean {
    switch (c) {
      case "Zero": return count === 0;
      case "One": return count === 1;
      case "ZeroOrOne": return count <= 1;
      case "OneOrMore": return count >= 1;
      case "ZeroOrMore": return true;
    }
  },

  compose(outer: Cardinality, inner: Cardinality): Cardinality {
    if (outer === "Zero" || inner === "Zero") return "Zero";
    if (outer === "One") return inner;
    if (outer === "ZeroOrOne") {
      if (inner === "One") return "ZeroOrOne";
      if (inner === "ZeroOrOne") return "ZeroOrOne";
      return "ZeroOrMore";
    }
    if (outer === "OneOrMore") {
      if (inner === "One") return "OneOrMore";
      if (inner === "OneOrMore") return "OneOrMore";
      return "ZeroOrMore";
    }
    return "ZeroOrMore";
  },
};

export interface StreamType {
  item: JTypeT;
  card: Cardinality;
}

export const StreamType = {
  new(item: JTypeT, card: Cardinality): StreamType {
    if (card === "Zero") return { item: "Never", card };
    return { item, card };
  },

  one(item: JTypeT): StreamType {
    return StreamType.new(item, "One");
  },

  zero(): StreamType {
    return StreamType.new("Never", "Zero");
  },

  zeroOrOne(item: JTypeT): StreamType {
    return StreamType.new(item, "ZeroOrOne");
  },

  zeroOrMore(item: JTypeT): StreamType {
    return StreamType.new(item, "ZeroOrMore");
  },

  join(a: StreamType, b: StreamType): StreamType {
    return StreamType.new(
      JType.union([a.item, b.item]),
      Cardinality.join(a.card, b.card),
    );
  },

  joinAlternative(a: StreamType, b: StreamType): StreamType {
    return StreamType.new(
      JType.union([a.item, b.item]),
      Cardinality.alternative(a.card, b.card),
    );
  },

  toCompactString(s: StreamType): string {
    if (s.card === "One") return JType.toCompactString(s.item);
    return `Stream<${JType.toCompactString(s.item)}, ${s.card}>`;
  },

  fitsOutputs(
    s: StreamType,
    outputs: unknown[],
    itemCheck: (value: unknown, ty: JTypeT) => boolean,
  ): boolean {
    if (!Cardinality.fitsCount(s.card, outputs.length)) return false;
    return outputs.every((value) => itemCheck(value, s.item));
  },
};
