export type BoolType = "Any" | { Literal: boolean };
export type NumberType = "Any" | { Literal: string };
export type StringType = "Any" | { Literal: string };

export interface ArrayType {
  items: JType;
}

export interface Property {
  ty: JType;
  required: boolean;
}

export interface ObjectType {
  properties: Record<string, Property>;
  additional: JType | null;
}

export type JType =
  | "Never"
  | "Unknown"
  | "Null"
  | { Bool: BoolType }
  | { Number: NumberType }
  | { String: StringType }
  | { Array: ArrayType }
  | { Object: ObjectType }
  | { Union: JType[] };

export type JTypeKind =
  | "Never"
  | "Unknown"
  | "Null"
  | "Bool"
  | "Number"
  | "String"
  | "Array"
  | "Object"
  | "Union";

export function jtypeKind(t: JType): JTypeKind {
  if (typeof t === "string") return t;
  if ("Bool" in t) return "Bool";
  if ("Number" in t) return "Number";
  if ("String" in t) return "String";
  if ("Array" in t) return "Array";
  if ("Object" in t) return "Object";
  return "Union";
}

export const JType = {
  Never: "Never" as JType,
  Unknown: "Unknown" as JType,
  Null: "Null" as JType,

  bool(): JType {
    return { Bool: "Any" };
  },

  boolLit(value: boolean): JType {
    return { Bool: { Literal: value } };
  },

  number(): JType {
    return { Number: "Any" };
  },

  numberLit(value: string | number): JType {
    return { Number: { Literal: String(value) } };
  },

  string(): JType {
    return { String: "Any" };
  },

  stringLit(value: string): JType {
    return { String: { Literal: value } };
  },

  array(items: JType): JType {
    return { Array: { items } };
  },

  object(properties: Record<string, Property>, additional: JType | null): JType {
    return { Object: { properties: sortedProperties(properties), additional } };
  },

  closedObject(properties: Record<string, Property>): JType {
    return JType.object(properties, null);
  },

  openObject(properties: Record<string, Property>): JType {
    return JType.object(properties, "Unknown");
  },

  property(ty: JType, required: boolean): Property {
    return { ty, required };
  },

  union(items: Iterable<JType>): JType {
    const flat: JType[] = [];
    for (const item of items) {
      if (item === "Never") continue;
      if (item === "Unknown") return "Unknown";
      if (typeof item === "object" && "Union" in item) {
        flat.push(...item.Union);
      } else {
        flat.push(item);
      }
    }

    if (flat.length === 0) return "Never";

    const seen = new Set<string>();
    const deduped: JType[] = [];
    for (const item of flat) {
      const key = toCompactString(item);
      if (!seen.has(key)) {
        seen.add(key);
        deduped.push(item);
      }
    }
    deduped.sort((a, b) => {
      const sa = toCompactString(a);
      const sb = toCompactString(b);
      return sa < sb ? -1 : sa > sb ? 1 : 0;
    });

    if (deduped.length === 1) return deduped[0]!;
    return { Union: deduped };
  },

  isNever(t: JType): boolean {
    return t === "Never";
  },

  isTruthyLiteral(t: JType): boolean | null {
    if (t === "Null" || t === "Never") return false;
    if (t === "Unknown") return null;
    if (typeof t === "object") {
      if ("Bool" in t) {
        if (t.Bool === "Any") return null;
        return t.Bool.Literal;
      }
      if ("Union" in t) return null;
    }
    return true;
  },

  typeNames(t: JType): string[] {
    if (t === "Never") return [];
    if (t === "Unknown") {
      return ["null", "boolean", "number", "string", "array", "object"];
    }
    if (t === "Null") return ["null"];
    if (typeof t === "object") {
      if ("Bool" in t) return ["boolean"];
      if ("Number" in t) return ["number"];
      if ("String" in t) return ["string"];
      if ("Array" in t) return ["array"];
      if ("Object" in t) return ["object"];
      const set = new Set<string>();
      for (const inner of t.Union) {
        for (const name of JType.typeNames(inner)) set.add(name);
      }
      return [...set].sort();
    }
    return [];
  },

  withoutNull(t: JType): JType {
    if (t === "Null") return "Never";
    if (typeof t === "object" && "Union" in t) {
      return JType.union(
        t.Union.map(JType.withoutNull).filter((item) => item !== "Never"),
      );
    }
    return t;
  },

  toCompactString(t: JType): string {
    return toCompactString(t);
  },
};

export function toCompactString(t: JType): string {
  if (t === "Never") return "empty";
  if (t === "Unknown") return "unknown";
  if (t === "Null") return "null";
  if ("Bool" in t) {
    return t.Bool === "Any" ? "boolean" : t.Bool.Literal ? "true" : "false";
  }
  if ("Number" in t) {
    return t.Number === "Any" ? "number" : t.Number.Literal;
  }
  if ("String" in t) {
    return t.String === "Any" ? "string" : JSON.stringify(t.String.Literal);
  }
  if ("Array" in t) {
    return `array<${toCompactString(t.Array.items)}>`;
  }
  if ("Object" in t) {
    return objectToCompactString(t.Object);
  }
  return t.Union.map(toCompactString).join(" | ");
}

function objectToCompactString(o: ObjectType): string {
  const parts: string[] = [];
  for (const key of Object.keys(o.properties).sort()) {
    const prop = o.properties[key]!;
    const suffix = prop.required ? "" : "?";
    parts.push(`${key}${suffix}: ${toCompactString(prop.ty)}`);
  }
  if (o.additional !== null) {
    if (o.additional === "Unknown") parts.push("...");
    else parts.push(`...: ${toCompactString(o.additional)}`);
  }
  return `object{${parts.join(", ")}}`;
}

function sortedProperties(properties: Record<string, Property>): Record<string, Property> {
  const sorted: Record<string, Property> = {};
  for (const key of Object.keys(properties).sort()) {
    sorted[key] = properties[key]!;
  }
  return sorted;
}
