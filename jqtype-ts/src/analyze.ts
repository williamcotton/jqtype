import { Diagnostic, type SourceSpan } from "./diagnostic.js";
import {
  jsonSchemaToType,
  sampleToType,
  typeToJsonSchema,
} from "./schema.js";
import { Cardinality, StreamType } from "./stream.js";
import {
  JType,
  type ObjectType,
  type Property,
  type JType as JTypeT,
} from "./types.js";
import {
  parseFilter,
  type ExpressionAst,
  type IfAst,
  type IndexAst,
  type IteratorAst,
  type ObjectEntryAst,
  type ProgAst,
  type SliceAst,
  type StrAst,
  type TryAst,
} from "./parser.js";

export type AnalysisMode = "Permissive" | "Strict";
export type OutputFormat = "Compact" | "JsonSchema" | "Tree";

export interface AnalyzeOptions {
  mode: AnalysisMode;
  source_name: string | null;
  output_format: OutputFormat;
  max_union_members: number;
  max_recursion_depth: number;
}

export const AnalyzeOptions = {
  default(): AnalyzeOptions {
    return {
      mode: "Permissive",
      source_name: null,
      output_format: "Compact",
      max_union_members: 32,
      max_recursion_depth: 32,
    };
  },
};

export interface UnsupportedFeature {
  feature: string;
  span: SourceSpan | null;
}

export interface AnalyzeReport {
  output: StreamType;
  diagnostics: Diagnostic[];
  unsupported_features: UnsupportedFeature[];
  debug_ast: string | null;
}

export const AnalyzeReport = {
  outputType(report: AnalyzeReport): StreamType {
    return report.output;
  },
  hasErrors(report: AnalyzeReport): boolean {
    return report.diagnostics.some((d) => d.severity === "Error");
  },
  toJsonSchemaValue(report: AnalyzeReport): unknown {
    return {
      stream: Cardinality.asStr(report.output.card),
      schema: typeToJsonSchema(report.output.item),
    };
  },
};

export type InputShape =
  | { Type: JTypeT }
  | { JsonSchema: unknown }
  | { Sample: unknown }
  | "Unknown";

export const InputShape = {
  fromType(ty: JTypeT): InputShape {
    return { Type: ty };
  },
  fromJsonSchema(schema: unknown): InputShape {
    return { JsonSchema: schema };
  },
  fromSample(sample: unknown): InputShape {
    return { Sample: sample };
  },
  unknown(): InputShape {
    return "Unknown";
  },
  intoType(input: InputShape): JTypeT {
    if (input === "Unknown") return "Unknown";
    if ("Type" in input) return input.Type;
    if ("JsonSchema" in input) return jsonSchemaToType(input.JsonSchema);
    return sampleToType(input.Sample);
  },
};

interface PredicateRefinement {
  whenTrue: JTypeT;
  whenFalse: JTypeT;
}

export class JqTypeChecker {
  parseDebugAst(source: string): { ok: true; ast: string } | { ok: false; report: AnalyzeReport } {
    const parsed = parseFilter(source);
    if (!parsed.ok) {
      return { ok: false, report: parseFailureReport(parsed.message, source, null) };
    }
    return { ok: true, ast: JSON.stringify(parsed.ast, null, 2) };
  }

  analyzeFilter(
    source: string,
    input: InputShape,
    options: AnalyzeOptions,
  ): AnalyzeReport {
    const sourceName = options.source_name;
    const parsed = parseFilter(source);
    if (!parsed.ok) {
      return parseFailureReport(parsed.message, source, sourceName);
    }

    const debugAst =
      options.output_format === "Tree"
        ? JSON.stringify(parsed.ast, null, 2)
        : null;

    const analyzer = new Analyzer(options);
    const inputTy = InputShape.intoType(input);
    const output = analyzer.analyzeProg(parsed.ast, inputTy);
    return {
      output,
      diagnostics: analyzer.diagnostics,
      unsupported_features: analyzer.unsupportedFeatures,
      debug_ast: debugAst,
    };
  }
}

function parseFailureReport(
  message: string,
  source: string,
  sourceName: string | null,
): AnalyzeReport {
  return {
    output: StreamType.zero(),
    diagnostics: [
      Diagnostic.withSourceName(
        Diagnostic.error(`failed to parse jq filter: ${message}`, {
          start: 0,
          end: source.length,
        }),
        sourceName,
      ),
    ],
    unsupported_features: [],
    debug_ast: null,
  };
}

class Analyzer {
  options: AnalyzeOptions;
  diagnostics: Diagnostic[] = [];
  unsupportedFeatures: UnsupportedFeature[] = [];

  constructor(options: AnalyzeOptions) {
    this.options = options;
  }

  analyzeProg(prog: ProgAst, input: JTypeT): StreamType {
    if (!prog.expr) return StreamType.one(input);
    return this.analyze(prog.expr, input);
  }

  analyze(expr: ExpressionAst, input: JTypeT): StreamType {
    switch (expr.type) {
      case "identity":
        return StreamType.one(input);
      case "num":
        return StreamType.one(JType.numberLit(numberLiteral(expr.value)));
      case "str":
        return StreamType.one(this.stringType(expr));
      case "bool":
        return StreamType.one(JType.boolLit(expr.value));
      case "null":
        return StreamType.one("Null");
      case "array":
        if (expr.expr === undefined) return StreamType.one(JType.array("Never"));
        return StreamType.one(JType.array(this.analyze(expr.expr, input).item));
      case "object":
        return this.analyzeObject(expr.entries, input);
      case "index":
        return this.analyzeIndex(expr, input, false);
      case "iterator":
        return this.analyzeIterator(expr, input, false);
      case "slice":
        return this.analyzeSlice(expr, input, false);
      case "binary":
        return this.analyzeBinary(expr, input);
      case "if":
        return this.analyzeIf(expr, input);
      case "filter":
        return this.analyzeCall(filterIdent(expr.name), expr.args, input);
      case "try":
        return this.analyzeTry(expr, input);
      case "unary":
        return this.analyzeUnary(expr, input);
      case "var":
        return this.unsupported(
          `variables are not supported yet: $${expr.name}`,
          input === "Unknown" ? "Unknown" : "Unknown",
        );
      case "varDeclaration":
        return this.unsupported(
          "variable bindings (`as`) are not supported yet",
          input,
        );
      case "reduce":
      case "foreach":
        return this.unsupported("fold filters are not supported yet", input);
      case "recursiveDescent":
        return this.unsupported(
          "recursive descent is not supported yet",
          input,
        );
      case "def":
        return this.unsupported(
          "function definitions are not supported yet",
          "Unknown",
        );
      case "label":
      case "break":
        return this.unsupported(
          "labels and break are not supported yet",
          input,
        );
      case "format":
        return this.unsupported("formats are not supported yet", input);
    }
  }

  analyzeObject(entries: ObjectEntryAst[], input: JTypeT): StreamType {
    const properties: Record<string, Property> = {};
    let additional: JTypeT | null = null;

    for (const entry of entries) {
      if (entry.value === undefined) {
        if (typeof entry.key === "string") {
          const value = this.accessField(input, entry.key, false).item;
          properties[entry.key] = { ty: value, required: true };
        } else {
          additional = "Unknown";
        }
      } else {
        const value = this.analyze(entry.value, input).item;
        const literalKey =
          typeof entry.key === "string"
            ? entry.key
            : entry.key.type === "str" && entry.key.interpolated === false
              ? entry.key.value
              : null;
        if (literalKey !== null) {
          properties[literalKey] = { ty: value, required: true };
        } else {
          additional =
            additional === null ? value : JType.union([additional, value]);
        }
      }
    }

    return StreamType.one(JType.object(properties, additional));
  }

  analyzeIndex(
    node: IndexAst,
    input: JTypeT,
    optional: boolean,
  ): StreamType {
    const inner = this.analyze(node.expr, input);
    const part = this.indexOp(inner.item, node.index, optional);
    return StreamType.new(part.item, Cardinality.compose(inner.card, part.card));
  }

  analyzeIterator(
    node: IteratorAst,
    input: JTypeT,
    optional: boolean,
  ): StreamType {
    const inner = this.analyze(node.expr, input);
    const part = this.iterate(inner.item, optional);
    return StreamType.new(part.item, Cardinality.compose(inner.card, part.card));
  }

  analyzeSlice(
    node: SliceAst,
    input: JTypeT,
    _optional: boolean,
  ): StreamType {
    const inner = this.analyze(node.expr, input);
    const part = this.slice(inner.item);
    return StreamType.new(part.item, Cardinality.compose(inner.card, part.card));
  }

  indexOp(input: JTypeT, index: string | ExpressionAst, optional: boolean): StreamType {
    if (typeof index === "string") return this.accessField(input, index, optional);
    if (index.type === "num") {
      return this.accessIndex(input, optional);
    }
    if (index.type === "str" && index.interpolated === false) {
      return this.accessField(input, index.value, optional);
    }
    return this.accessDynamicIndex(input, optional);
  }

  accessField(input: JTypeT, key: string, optional: boolean): StreamType {
    if (typeof input === "object") {
      if ("Object" in input) {
        const prop = input.Object.properties[key];
        if (prop) {
          return StreamType.one(
            prop.required ? prop.ty : JType.union([prop.ty, "Null"]),
          );
        }
        if (input.Object.additional !== null) {
          return StreamType.one(JType.union([input.Object.additional, "Null"]));
        }
        return StreamType.one("Null");
      }
      if ("Union" in input) {
        let out = StreamType.zero();
        for (const item of input.Union) {
          out = StreamType.join(out, this.accessField(item, key, optional));
        }
        return out;
      }
    }
    if (input === "Null") return StreamType.one("Null");
    if (input === "Unknown") return StreamType.one("Unknown");

    if (optional) {
      this.warnOrError(
        `optional field \`${key}\` skipped non-object input: ${JType.toCompactString(input)}`,
      );
      return StreamType.zero();
    }
    this.warnOrError(
      `field \`${key}\` may be applied to non-object input: ${JType.toCompactString(input)}`,
    );
    return StreamType.one("Unknown");
  }

  accessIndex(input: JTypeT, optional: boolean): StreamType {
    if (typeof input === "object") {
      if ("Array" in input) {
        return StreamType.one(JType.union([input.Array.items, "Null"]));
      }
      if ("Union" in input) {
        let out = StreamType.zero();
        for (const item of input.Union) {
          out = StreamType.join(out, this.accessIndex(item, optional));
        }
        return out;
      }
    }
    if (input === "Unknown") return StreamType.one("Unknown");

    if (optional) {
      this.warnOrError(
        `optional index skipped non-array input: ${JType.toCompactString(input)}`,
      );
      return StreamType.zero();
    }
    this.warnOrError(
      `array index may be applied to non-array input: ${JType.toCompactString(input)}`,
    );
    return StreamType.one("Unknown");
  }

  accessDynamicIndex(input: JTypeT, optional: boolean): StreamType {
    if (typeof input === "object") {
      if ("Array" in input) {
        return StreamType.one(JType.union([input.Array.items, "Null"]));
      }
      if ("Object" in input) {
        const values: JTypeT[] = Object.values(input.Object.properties).map(
          (p) => p.ty,
        );
        if (input.Object.additional !== null) values.push(input.Object.additional);
        return StreamType.one(JType.union(values));
      }
      if ("Union" in input) {
        let out = StreamType.zero();
        for (const item of input.Union) {
          out = StreamType.join(out, this.accessDynamicIndex(item, optional));
        }
        return out;
      }
    }
    if (input === "Unknown") return StreamType.one("Unknown");

    if (optional) {
      this.warnOrError(
        `optional dynamic index skipped input: ${JType.toCompactString(input)}`,
      );
      return StreamType.zero();
    }
    this.warnOrError(
      `dynamic index may be applied to non-container input: ${JType.toCompactString(input)}`,
    );
    return StreamType.one("Unknown");
  }

  iterate(input: JTypeT, optional: boolean): StreamType {
    if (typeof input === "object") {
      if ("Array" in input) {
        return StreamType.zeroOrMore(input.Array.items);
      }
      if ("Object" in input) {
        const values: JTypeT[] = Object.values(input.Object.properties).map(
          (p) => p.ty,
        );
        if (input.Object.additional !== null) values.push(input.Object.additional);
        return StreamType.zeroOrMore(JType.union(values));
      }
      if ("Union" in input) {
        let out = StreamType.zero();
        for (const item of input.Union) {
          out = StreamType.join(out, this.iterate(item, optional));
        }
        return out;
      }
    }
    if (input === "Unknown") return StreamType.zeroOrMore("Unknown");

    if (optional) {
      this.warnOrError(
        `optional iteration skipped non-iterable input: ${JType.toCompactString(input)}`,
      );
      return StreamType.zero();
    }
    this.warnOrError(
      `iteration may be applied to non-iterable input: ${JType.toCompactString(input)}`,
    );
    return StreamType.zeroOrMore("Unknown");
  }

  slice(input: JTypeT): StreamType {
    if (typeof input === "object") {
      if ("Array" in input) return StreamType.one(JType.array(input.Array.items));
      if ("String" in input) return StreamType.one(JType.string());
      if ("Union" in input) {
        return StreamType.one(
          JType.union(input.Union.map((item) => this.slice(item).item)),
        );
      }
    }
    if (input === "Unknown") return StreamType.one("Unknown");
    this.warnOrError(
      `slice may be applied to non-array/non-string input: ${JType.toCompactString(input)}`,
    );
    return StreamType.one("Unknown");
  }

  analyzeBinary(
    node: { left: ExpressionAst; right: ExpressionAst; operator: string },
    input: JTypeT,
  ): StreamType {
    const op = node.operator;
    if (op === "|") {
      const left = this.analyze(node.left, input);
      if (left.card === "Zero") return StreamType.zero();
      let perItem: StreamType | null = null;
      for (const item of flattenUnion(left.item)) {
        const branch = this.analyze(node.right, item);
        perItem = perItem === null ? branch : StreamType.joinAlternative(perItem, branch);
      }
      const composed = perItem ?? StreamType.zero();
      return StreamType.new(composed.item, Cardinality.compose(left.card, composed.card));
    }
    if (op === ",") {
      return StreamType.join(
        this.analyze(node.left, input),
        this.analyze(node.right, input),
      );
    }
    if (op === "==" || op === "!=" || op === "<" || op === ">" || op === "<=" || op === ">=") {
      return StreamType.one(JType.bool());
    }
    if (op === "or" || op === "and") return StreamType.one(JType.bool());
    if (op === "+" || op === "-" || op === "*" || op === "/" || op === "%") {
      this.analyze(node.left, input);
      this.analyze(node.right, input);
      return StreamType.one("Unknown");
    }
    if (op === "//") {
      const left = this.analyze(node.left, input);
      return StreamType.join(left, this.analyze(node.right, input));
    }
    return this.unsupported(
      `assignment operator \`${op}\` is not supported yet`,
      input,
    );
  }

  analyzeIf(node: IfAst, input: JTypeT): StreamType {
    const branches: { cond: ExpressionAst; then: ExpressionAst }[] = [
      { cond: node.cond, then: node.then },
      ...(node.elifs ?? []),
    ];

    let output: StreamType | null = null;
    let remaining: JTypeT = input;

    for (const { cond, then } of branches) {
      if (remaining === "Never") break;
      const refinement = this.analyzePredicate(cond, remaining);
      if (refinement.whenTrue !== "Never") {
        const branch = this.analyze(then, refinement.whenTrue);
        output = output === null ? branch : StreamType.joinAlternative(output, branch);
      }
      remaining = refinement.whenFalse;
    }

    if (node.else !== undefined && remaining !== "Never") {
      const branch = this.analyze(node.else, remaining);
      return output === null ? branch : StreamType.joinAlternative(output, branch);
    }
    return output ?? StreamType.zero();
  }

  analyzeTry(node: TryAst, input: JTypeT): StreamType {
    if (node.short) {
      if (node.body.type === "index") return this.analyzeIndex(node.body, input, true);
      if (node.body.type === "iterator") return this.analyzeIterator(node.body, input, true);
      if (node.body.type === "slice") return this.analyzeSlice(node.body, input, true);
    }
    const primary = this.analyze(node.body, input);
    if (node.catch !== undefined) {
      return StreamType.join(primary, this.analyze(node.catch, input));
    }
    return primary;
  }

  analyzeUnary(
    node: { operator: string; expr: ExpressionAst },
    input: JTypeT,
  ): StreamType {
    if (node.operator === "-" && node.expr.type === "num") {
      return StreamType.one(JType.numberLit(`-${numberLiteral(node.expr.value)}`));
    }
    this.analyze(node.expr, input);
    return StreamType.one(JType.number());
  }

  analyzeCall(name: string, args: ExpressionAst[], input: JTypeT): StreamType {
    if (name === "null" && args.length === 0) return StreamType.one("Null");
    if (name === "true" && args.length === 0) return StreamType.one(JType.boolLit(true));
    if (name === "false" && args.length === 0) return StreamType.one(JType.boolLit(false));
    if (name === "empty" && args.length === 0) return StreamType.zero();
    if (name === "type" && args.length === 0) {
      return StreamType.one(
        JType.union(JType.typeNames(input).map((kind) => JType.stringLit(kind))),
      );
    }
    if (name === "length" && args.length === 0) return StreamType.one(JType.number());
    if (name === "keys" && args.length === 0) return StreamType.one(this.keysType(input));
    if (name === "not" && args.length === 0) return StreamType.one(JType.bool());
    if (name === "has" && args.length === 1) {
      return StreamType.one(this.hasType(input, args[0]!));
    }
    if (name === "select" && args.length === 1) {
      const refinement = this.analyzePredicate(args[0]!, input);
      if (refinement.whenTrue === "Never") return StreamType.zero();
      if (refinement.whenFalse === "Never") return StreamType.one(refinement.whenTrue);
      return StreamType.zeroOrOne(refinement.whenTrue);
    }
    if (name === "map" && args.length === 1) {
      return this.mapCall(args[0]!, input);
    }
    if (name === "values" && args.length === 0) return this.filterValues(input);
    if (name === "nulls" && args.length === 0) return this.filterKind(input, "null");
    if (name === "booleans" && args.length === 0) return this.filterKind(input, "boolean");
    if (name === "numbers" && args.length === 0) return this.filterKind(input, "number");
    if (name === "strings" && args.length === 0) return this.filterKind(input, "string");
    if (name === "arrays" && args.length === 0) return this.filterKind(input, "array");
    if (name === "objects" && args.length === 0) return this.filterKind(input, "object");
    return this.unsupported(`unsupported builtin or call \`${name}\``, "Unknown");
  }

  analyzePredicate(expr: ExpressionAst, input: JTypeT): PredicateRefinement {
    if (expr.type === "binary") {
      const op = expr.operator;
      if (op === "==" || op === "!=") {
        const tk1 = typeComparisonKind(expr.left, expr.right);
        if (tk1 !== null) return refineTypePredicate(input, tk1, op);
        const tk2 = typeComparisonKind(expr.right, expr.left);
        if (tk2 !== null) return refineTypePredicate(input, tk2, op);

        const leftField = topLevelFieldAccess(expr.left);
        const rightLit = literalTypeFilter(expr.right);
        if (leftField !== null && rightLit !== null) {
          return refineFieldLiteralPredicate(input, leftField, rightLit, op);
        }
        const rightField = topLevelFieldAccess(expr.right);
        const leftLit = literalTypeFilter(expr.left);
        if (rightField !== null && leftLit !== null) {
          return refineFieldLiteralPredicate(input, rightField, leftLit, op);
        }
      } else if (op === "and") {
        const left = this.analyzePredicate(expr.left, input);
        const right = this.analyzePredicate(expr.right, left.whenTrue);
        return {
          whenTrue: right.whenTrue,
          whenFalse: JType.union([left.whenFalse, right.whenFalse]),
        };
      } else if (op === "or") {
        const left = this.analyzePredicate(expr.left, input);
        const right = this.analyzePredicate(expr.right, left.whenFalse);
        return {
          whenTrue: JType.union([left.whenTrue, right.whenTrue]),
          whenFalse: right.whenFalse,
        };
      }
    } else if (expr.type === "filter" && filterIdent(expr.name) === "has" && expr.args.length === 1) {
      const key = literalStringFilter(expr.args[0]!);
      if (key !== null) return refineHasPredicate(input, key);
    }

    const output = this.analyze(expr, input);
    const truthy = JType.isTruthyLiteral(output.item);
    if (truthy === true) return { whenTrue: input, whenFalse: "Never" };
    if (truthy === false) return { whenTrue: "Never", whenFalse: input };
    return { whenTrue: input, whenFalse: input };
  }

  hasType(input: JTypeT, key: ExpressionAst): JTypeT {
    const lit = literalStringFilter(key);
    if (lit === null) return JType.bool();
    const refinement = refineHasPredicate(input, lit);
    const trueImpossible = refinement.whenTrue === "Never";
    const falseImpossible = refinement.whenFalse === "Never";
    if (trueImpossible && !falseImpossible) return JType.boolLit(false);
    if (!trueImpossible && falseImpossible) return JType.boolLit(true);
    return JType.bool();
  }

  keysType(input: JTypeT): JTypeT {
    if (typeof input === "object") {
      if ("Object" in input) {
        const keys: JTypeT[] = Object.keys(input.Object.properties).map((k) =>
          JType.stringLit(k),
        );
        if (input.Object.additional !== null) keys.push(JType.string());
        return JType.array(JType.union(keys));
      }
      if ("Array" in input) return JType.array(JType.number());
      if ("Union" in input) {
        return JType.union(input.Union.map((item) => this.keysType(item)));
      }
    }
    if (input === "Unknown") {
      return JType.array(JType.union([JType.string(), JType.number()]));
    }
    this.warnOrError(
      `keys may be applied to non-array/non-object input: ${JType.toCompactString(input)}`,
    );
    return "Unknown";
  }

  mapCall(mapper: ExpressionAst, input: JTypeT): StreamType {
    if (typeof input === "object") {
      if ("Array" in input) {
        const mapped = this.analyze(mapper, input.Array.items);
        return StreamType.one(JType.array(mapped.item));
      }
      if ("Union" in input) {
        return StreamType.one(
          JType.union(input.Union.map((item) => this.mapCall(mapper, item).item)),
        );
      }
    }
    if (input === "Unknown") return StreamType.one(JType.array("Unknown"));
    this.warnOrError(
      `map may be applied to non-array input: ${JType.toCompactString(input)}`,
    );
    return StreamType.one("Unknown");
  }

  filterValues(input: JTypeT): StreamType {
    const nonNull = JType.withoutNull(input);
    if (nonNull === "Never") return StreamType.zero();
    return StreamType.zeroOrOne(nonNull);
  }

  filterKind(input: JTypeT, kind: string): StreamType {
    const matching = narrowByTypeName(input, kind);
    if (matching === "Never") return StreamType.zero();
    if (excludeByTypeName(input, kind) === "Never") return StreamType.one(matching);
    return StreamType.zeroOrOne(matching);
  }

  stringType(value: StrAst): JTypeT {
    const lit = literalString(value);
    return lit === null ? JType.string() : JType.stringLit(lit);
  }

  unsupported(feature: string, fallback: JTypeT): StreamType {
    this.unsupportedFeatures.push({ feature, span: null });
    this.warnOrError(feature);
    return StreamType.one(fallback);
  }

  warnOrError(message: string, span: SourceSpan | null = null): void {
    const base =
      this.options.mode === "Strict"
        ? Diagnostic.error(message, span)
        : Diagnostic.warning(message, span);
    this.diagnostics.push(
      Diagnostic.withSourceName(base, this.options.source_name),
    );
  }
}

function literalString(value: StrAst): string | null {
  if (value.format !== undefined) return null;
  if (value.interpolated === false) return value.value;
  let out = "";
  for (const part of value.parts) {
    if (typeof part === "string") out += part;
    else return null;
  }
  return out;
}

function literalStringFilter(expr: ExpressionAst): string | null {
  if (expr.type !== "str") return null;
  return literalString(expr);
}

function literalTypeFilter(expr: ExpressionAst): JTypeT | null {
  if (expr.type === "null") return "Null";
  if (expr.type === "bool") return JType.boolLit(expr.value);
  if (expr.type === "str") {
    const lit = literalString(expr);
    return lit === null ? null : JType.stringLit(lit);
  }
  if (expr.type === "num") {
    return JType.numberLit(numberLiteral(expr.value));
  }
  if (expr.type === "filter" && expr.args.length === 0) {
    const ident = filterIdent(expr.name);
    if (ident === "null") return "Null";
    if (ident === "true") return JType.boolLit(true);
    if (ident === "false") return JType.boolLit(false);
  }
  if (expr.type === "unary" && expr.operator === "-" && expr.expr.type === "num") {
    return JType.numberLit(`-${numberLiteral(expr.expr.value)}`);
  }
  return null;
}

function typeComparisonKind(
  typeFilter: ExpressionAst,
  literal: ExpressionAst,
): string | null {
  if (typeFilter.type !== "filter") return null;
  if (filterIdent(typeFilter.name) !== "type" || typeFilter.args.length !== 0) return null;
  const lit = literalTypeFilter(literal);
  if (lit === null) return null;
  if (typeof lit === "object" && "String" in lit && lit.String !== "Any") {
    return lit.String.Literal;
  }
  return null;
}

function topLevelFieldAccess(expr: ExpressionAst): string | null {
  if (expr.type !== "index") return null;
  if (expr.expr.type !== "identity") return null;
  if (typeof expr.index !== "string") {
    if (
      typeof expr.index === "object" &&
      expr.index.type === "str" &&
      expr.index.interpolated === false
    ) {
      return expr.index.value;
    }
    return null;
  }
  return expr.index;
}

function refineHasPredicate(input: JTypeT, key: string): PredicateRefinement {
  if (typeof input === "object") {
    if ("Object" in input) {
      const obj = input.Object;
      const prop = obj.properties[key];
      if (prop) {
        if (prop.required) {
          return { whenTrue: input, whenFalse: "Never" };
        }
        const trueProps = { ...obj.properties, [key]: { ...prop, required: true } };
        const falseProps: Record<string, Property> = { ...obj.properties };
        delete falseProps[key];
        return {
          whenTrue: JType.object(trueProps, obj.additional),
          whenFalse: JType.object(falseProps, obj.additional),
        };
      }
      if (obj.additional !== null) {
        const trueProps: Record<string, Property> = {
          ...obj.properties,
          [key]: { ty: "Unknown", required: true },
        };
        return {
          whenTrue: JType.object(trueProps, obj.additional),
          whenFalse: input,
        };
      }
      return { whenTrue: "Never", whenFalse: input };
    }
    if ("Union" in input) {
      const refinements = input.Union.map((item) => refineHasPredicate(item, key));
      return {
        whenTrue: JType.union(refinements.map((r) => r.whenTrue)),
        whenFalse: JType.union(refinements.map((r) => r.whenFalse)),
      };
    }
  }
  if (input === "Unknown") {
    return {
      whenTrue: JType.openObject({ [key]: { ty: "Unknown", required: true } }),
      whenFalse: "Unknown",
    };
  }
  return { whenTrue: "Never", whenFalse: input };
}

function refineFieldEquals(
  input: JTypeT,
  field: string,
  literal: JTypeT,
): PredicateRefinement {
  if (typeof input === "object") {
    if ("Object" in input) {
      const obj = input.Object;
      const prop = obj.properties[field];
      if (prop) {
        const trueTy = intersectType(prop.ty, literal);
        const falseTy = excludeLiteralType(prop.ty, literal);

        const whenTrue: JTypeT =
          trueTy === "Never"
            ? "Never"
            : JType.object(
                { ...obj.properties, [field]: { ty: trueTy, required: true } },
                obj.additional,
              );

        let whenFalse: JTypeT;
        if (falseTy === "Never" && prop.required) {
          whenFalse = "Never";
        } else {
          const props: Record<string, Property> = { ...obj.properties };
          if (falseTy === "Never") delete props[field];
          else props[field] = { ty: falseTy, required: prop.required };
          whenFalse = JType.object(props, obj.additional);
        }
        return { whenTrue, whenFalse };
      }
      if (obj.additional !== null) {
        const whenTrue =
          literal === "Null"
            ? input
            : JType.object(
                { ...obj.properties, [field]: { ty: literal, required: true } },
                obj.additional,
              );
        return { whenTrue, whenFalse: input };
      }
      if (literal === "Null") return { whenTrue: input, whenFalse: "Never" };
      return { whenTrue: "Never", whenFalse: input };
    }
    if ("Union" in input) {
      const refinements = input.Union.map((item) =>
        refineFieldEquals(item, field, literal),
      );
      return {
        whenTrue: JType.union(refinements.map((r) => r.whenTrue)),
        whenFalse: JType.union(refinements.map((r) => r.whenFalse)),
      };
    }
  }
  if (input === "Null" && literal === "Null") {
    return { whenTrue: "Null", whenFalse: "Never" };
  }
  if (input === "Unknown") {
    return {
      whenTrue: JType.openObject({ [field]: { ty: literal, required: true } }),
      whenFalse: "Unknown",
    };
  }
  return { whenTrue: "Never", whenFalse: input };
}

function refineFieldNonNull(input: JTypeT, field: string): PredicateRefinement {
  if (typeof input === "object") {
    if ("Object" in input) {
      const obj = input.Object;
      const prop = obj.properties[field];
      if (prop) {
        const nonNull = JType.withoutNull(prop.ty);
        const nullPart = intersectType(prop.ty, "Null");

        const whenTrue: JTypeT =
          nonNull === "Never"
            ? "Never"
            : JType.object(
                { ...obj.properties, [field]: { ty: nonNull, required: true } },
                obj.additional,
              );

        let whenFalse: JTypeT;
        if (prop.required && nullPart === "Never") {
          whenFalse = "Never";
        } else {
          const props: Record<string, Property> = { ...obj.properties };
          if (nullPart === "Never") delete props[field];
          else props[field] = { ty: "Null", required: prop.required };
          whenFalse = JType.object(props, obj.additional);
        }
        return { whenTrue, whenFalse };
      }
      if (obj.additional !== null) {
        const whenTrue = JType.object(
          { ...obj.properties, [field]: { ty: "Unknown", required: true } },
          obj.additional,
        );
        return { whenTrue, whenFalse: input };
      }
      return { whenTrue: "Never", whenFalse: input };
    }
    if ("Union" in input) {
      const refinements = input.Union.map((item) => refineFieldNonNull(item, field));
      return {
        whenTrue: JType.union(refinements.map((r) => r.whenTrue)),
        whenFalse: JType.union(refinements.map((r) => r.whenFalse)),
      };
    }
  }
  if (input === "Unknown") {
    return {
      whenTrue: JType.openObject({ [field]: { ty: "Unknown", required: true } }),
      whenFalse: "Unknown",
    };
  }
  return { whenTrue: "Never", whenFalse: input };
}

function refineFieldLiteralPredicate(
  input: JTypeT,
  field: string,
  literal: JTypeT,
  op: "==" | "!=",
): PredicateRefinement {
  if (literal === "Null") {
    const nonNull = refineFieldNonNull(input, field);
    if (op === "!=") return nonNull;
    return { whenTrue: nonNull.whenFalse, whenFalse: nonNull.whenTrue };
  }
  const eq = refineFieldEquals(input, field, literal);
  if (op === "==") return eq;
  return { whenTrue: eq.whenFalse, whenFalse: eq.whenTrue };
}

function refineTypePredicate(
  input: JTypeT,
  kind: string,
  op: "==" | "!=",
): PredicateRefinement {
  const matching = narrowByTypeName(input, kind);
  const nonMatching = excludeByTypeName(input, kind);
  if (op === "==") return { whenTrue: matching, whenFalse: nonMatching };
  return { whenTrue: nonMatching, whenFalse: matching };
}

function intersectType(ty: JTypeT, literal: JTypeT): JTypeT {
  if (ty === "Unknown") return literal;
  if (typeof ty === "object" && "Union" in ty) {
    return JType.union(ty.Union.map((item) => intersectType(item, literal)));
  }
  if (ty === "Null" && literal === "Null") return "Null";
  if (typeof ty === "object" && typeof literal === "object") {
    if ("Bool" in ty && "Bool" in literal) {
      if (ty.Bool === "Any") return literal;
      if (literal.Bool === "Any") return ty;
      if (ty.Bool.Literal === literal.Bool.Literal) return JType.boolLit(ty.Bool.Literal);
    }
    if ("Number" in ty && "Number" in literal) {
      if (ty.Number === "Any") return literal;
      if (literal.Number === "Any") return ty;
      if (ty.Number.Literal === literal.Number.Literal) {
        return JType.numberLit(ty.Number.Literal);
      }
    }
    if ("String" in ty && "String" in literal) {
      if (ty.String === "Any") return literal;
      if (literal.String === "Any") return ty;
      if (ty.String.Literal === literal.String.Literal) {
        return JType.stringLit(ty.String.Literal);
      }
    }
  }
  if (JType.toCompactString(ty) === JType.toCompactString(literal)) return ty;
  return "Never";
}

function excludeLiteralType(ty: JTypeT, literal: JTypeT): JTypeT {
  if (typeof ty === "object" && "Union" in ty) {
    return JType.union(ty.Union.map((item) => excludeLiteralType(item, literal)));
  }
  if (intersectType(ty, literal) === "Never") return ty;
  if (ty === "Null") return "Never";
  if (typeof ty === "object") {
    if ("Bool" in ty && ty.Bool !== "Any") return "Never";
    if ("Number" in ty && ty.Number !== "Any") return "Never";
    if ("String" in ty && ty.String !== "Any") return "Never";
  }
  return ty;
}

function narrowByTypeName(input: JTypeT, kind: string): JTypeT {
  if (input === "Unknown") return kindToType(kind);
  if (typeof input === "object" && "Union" in input) {
    return JType.union(input.Union.map((item) => narrowByTypeName(item, kind)));
  }
  if (JType.typeNames(input).includes(kind)) return input;
  return "Never";
}

function excludeByTypeName(input: JTypeT, kind: string): JTypeT {
  if (input === "Unknown") return "Unknown";
  if (typeof input === "object" && "Union" in input) {
    return JType.union(input.Union.map((item) => excludeByTypeName(item, kind)));
  }
  if (JType.typeNames(input).includes(kind)) return "Never";
  return input;
}

function kindToType(kind: string): JTypeT {
  switch (kind) {
    case "null": return "Null";
    case "boolean": return JType.bool();
    case "number": return JType.number();
    case "string": return JType.string();
    case "array": return JType.array("Unknown");
    case "object": return JType.openObject({});
    default: return "Never";
  }
}

function flattenUnion(input: JTypeT): JTypeT[] {
  if (typeof input === "object" && "Union" in input) return input.Union;
  return [input];
}

function numberLiteral(value: number): string {
  return Number.isInteger(value) ? String(value) : String(value);
}

function filterIdent(name: string): string {
  const slash = name.lastIndexOf("/");
  return slash === -1 ? name : name.slice(0, slash);
}

// Re-export ObjectType for downstream consumers that want to build host types.
export type { ObjectType };
