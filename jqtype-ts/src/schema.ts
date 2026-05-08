import { JType, type Property, type ObjectType } from "./types.js";

export function sampleToType(value: unknown): JType {
  if (value === null) return "Null";
  if (typeof value === "boolean") return JType.boolLit(value);
  if (typeof value === "number") return JType.numberLit(numberToString(value));
  if (typeof value === "string") return JType.stringLit(value);
  if (Array.isArray(value)) {
    return JType.array(JType.union(value.map(sampleToType)));
  }
  if (typeof value === "object") {
    const properties: Record<string, Property> = {};
    for (const [key, v] of Object.entries(value as Record<string, unknown>)) {
      properties[key] = { ty: sampleToType(v), required: true };
    }
    return JType.closedObject(properties);
  }
  return "Unknown";
}

export function jsonSchemaToType(schema: unknown): JType {
  if (!isPlainObject(schema)) return "Unknown";

  if (Array.isArray(schema.enum)) {
    return JType.union(schema.enum.map(sampleToType));
  }

  if (Array.isArray(schema.anyOf)) {
    return JType.union(schema.anyOf.map(jsonSchemaToType));
  }

  if (Array.isArray(schema.oneOf)) {
    return JType.union(schema.oneOf.map(jsonSchemaToType));
  }

  if (Array.isArray(schema.allOf)) {
    return mergeAllOf(schema.allOf);
  }

  const kind = schema.type;
  if (typeof kind === "string") return schemaTypeToType(kind, schema);
  if (Array.isArray(kind)) {
    return JType.union(
      kind
        .filter((k): k is string => typeof k === "string")
        .map((k) => schemaTypeToType(k, schema)),
    );
  }
  if ("properties" in schema) return objectSchemaToType(schema);
  if ("items" in schema) return arraySchemaToType(schema);
  return "Unknown";
}

export function typeToJsonSchema(ty: JType): unknown {
  if (ty === "Never") return false;
  if (ty === "Unknown") return {};
  if (ty === "Null") return { type: "null" };
  if ("Bool" in ty) {
    if (ty.Bool === "Any") return { type: "boolean" };
    return { const: ty.Bool.Literal };
  }
  if ("Number" in ty) {
    if (ty.Number === "Any") return { type: "number" };
    const literal = ty.Number.Literal;
    const parsed = Number(literal);
    return {
      const: Number.isFinite(parsed) ? parsed : literal,
    };
  }
  if ("String" in ty) {
    if (ty.String === "Any") return { type: "string" };
    return { const: ty.String.Literal };
  }
  if ("Array" in ty) {
    return { type: "array", items: typeToJsonSchema(ty.Array.items) };
  }
  if ("Object" in ty) {
    return objectToJsonSchema(ty.Object);
  }
  return { anyOf: ty.Union.map(typeToJsonSchema) };
}

export function valueFitsType(value: unknown, ty: JType): boolean {
  if (ty === "Never") return false;
  if (ty === "Unknown") return true;
  if (ty === "Null") return value === null;
  if ("Bool" in ty) {
    if (typeof value !== "boolean") return false;
    if (ty.Bool === "Any") return true;
    return value === ty.Bool.Literal;
  }
  if ("Number" in ty) {
    if (typeof value !== "number") return false;
    if (ty.Number === "Any") return true;
    return numberEq(value, ty.Number.Literal);
  }
  if ("String" in ty) {
    if (typeof value !== "string") return false;
    if (ty.String === "Any") return true;
    return value === ty.String.Literal;
  }
  if ("Array" in ty) {
    if (!Array.isArray(value)) return false;
    return value.every((item) => valueFitsType(item, ty.Array.items));
  }
  if ("Object" in ty) {
    if (!isPlainObject(value)) return false;
    return objectFits(value, ty.Object);
  }
  return ty.Union.some((member) => valueFitsType(value, member));
}

function schemaTypeToType(
  kind: string,
  schema: Record<string, unknown>,
): JType {
  switch (kind) {
    case "null": return "Null";
    case "boolean": return JType.bool();
    case "number":
    case "integer":
      return JType.number();
    case "string": return JType.string();
    case "array": return arraySchemaToType(schema);
    case "object": return objectSchemaToType(schema);
    default: return "Unknown";
  }
}

function arraySchemaToType(schema: Record<string, unknown>): JType {
  const items = schema.items;
  let item: JType;
  if (Array.isArray(items)) {
    item = JType.union(items.map(jsonSchemaToType));
  } else if (items !== undefined) {
    item = jsonSchemaToType(items);
  } else {
    item = "Unknown";
  }
  return JType.array(item);
}

function objectSchemaToType(schema: Record<string, unknown>): JType {
  const requiredList = Array.isArray(schema.required)
    ? schema.required.filter((k): k is string => typeof k === "string")
    : [];
  const required = new Set(requiredList);

  const properties: Record<string, Property> = {};
  if (isPlainObject(schema.properties)) {
    for (const [key, propSchema] of Object.entries(schema.properties)) {
      properties[key] = {
        ty: jsonSchemaToType(propSchema),
        required: required.has(key),
      };
    }
  }

  let additional: JType | null;
  const ap = schema.additionalProperties;
  if (ap === false) additional = null;
  else if (ap === true || ap === undefined) additional = "Unknown";
  else additional = jsonSchemaToType(ap);

  return JType.object(properties, additional);
}

function mergeAllOf(items: unknown[]): JType {
  const properties: Record<string, Property> = {};
  let additional: JType | null = "Unknown";

  for (const item of items) {
    const ty = jsonSchemaToType(item);
    if (typeof ty !== "object" || !("Object" in ty)) {
      return ty;
    }
    for (const [key, prop] of Object.entries(ty.Object.properties)) {
      const existing = properties[key];
      if (existing) {
        existing.ty = JType.union([existing.ty, prop.ty]);
        existing.required = existing.required || prop.required;
      } else {
        properties[key] = { ...prop };
      }
    }
    if (ty.Object.additional === null) additional = null;
  }

  return JType.object(properties, additional);
}

function objectToJsonSchema(object: ObjectType): unknown {
  const properties: Record<string, unknown> = {};
  const required: string[] = [];

  for (const key of Object.keys(object.properties).sort()) {
    const prop = object.properties[key]!;
    properties[key] = typeToJsonSchema(prop.ty);
    if (prop.required) required.push(key);
  }

  let additional: unknown;
  if (object.additional === null) additional = false;
  else if (object.additional === "Unknown") additional = true;
  else additional = typeToJsonSchema(object.additional);

  return {
    type: "object",
    properties,
    required,
    additionalProperties: additional,
  };
}

function objectFits(
  value: Record<string, unknown>,
  object: ObjectType,
): boolean {
  for (const [key, prop] of Object.entries(object.properties)) {
    if (key in value) {
      if (!valueFitsType(value[key], prop.ty)) return false;
    } else if (prop.required) {
      return false;
    }
  }

  if (object.additional !== null) {
    for (const [key, actual] of Object.entries(value)) {
      if (key in object.properties) continue;
      if (!valueFitsType(actual, object.additional)) return false;
    }
  } else {
    for (const key of Object.keys(value)) {
      if (!(key in object.properties)) return false;
    }
  }

  return true;
}

function numberEq(actual: number, expected: string): boolean {
  if (numberToString(actual) === expected) return true;
  const parsed = Number(expected);
  return Number.isFinite(parsed) && parsed === actual;
}

function numberToString(n: number): string {
  // Match serde_json's Number formatting: integers render without ".0".
  return Number.isInteger(n) && Object.is(n, Math.trunc(n))
    ? String(n)
    : String(n);
}

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}
