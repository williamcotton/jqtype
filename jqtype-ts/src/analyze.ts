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
  type ForeachAst,
  type IfAst,
  type IndexAst,
  type IteratorAst,
  type ObjectEntryAst,
  type ProgAst,
  type ReduceAst,
  type SliceAst,
  type StrAst,
  type TryAst,
  type VarDeclarationAst,
  type DestructuringAst,
  type ObjectDestructuringAst,
  type ArrayDestructuringAst,
  type DefAst,
  type ArgAst,
} from "./parser.js";

interface FilterArgBinding {
  expr: ExpressionAst;
  env: Map<string, JType>;
  filterArgs: Map<string, FilterArgBinding>;
  defs: Map<string, DefEntry>;
}

interface DefEntry {
  args: ArgAst[];
  body: ExpressionAst;
  capturedEnv: Map<string, JType>;
  capturedFilterArgs: Map<string, FilterArgBinding>;
  capturedDefs: Map<string, DefEntry>;
}

export type AnalysisMode = "Permissive" | "Strict";
export type OutputFormat = "Compact" | "JsonSchema" | "Tree";

export interface AnalyzeOptions {
  mode?: AnalysisMode;
  source_name?: string | null;
  output_format?: OutputFormat;
  max_union_members?: number;
  max_recursion_depth?: number;
  externalVars?: Record<string, JTypeT>;
}

interface ResolvedAnalyzeOptions {
  mode: AnalysisMode;
  source_name: string | null;
  output_format: OutputFormat;
  max_union_members: number;
  max_recursion_depth: number;
  externalVars: Record<string, JTypeT>;
}

export const AnalyzeOptions = {
  default(): AnalyzeOptions {
    return {
      mode: "Permissive",
      source_name: null,
      output_format: "Compact",
      max_union_members: 32,
      max_recursion_depth: 32,
      externalVars: {},
    };
  },
};

export type CapabilityStatus = "supported" | "partial" | "unsupported";

export interface JqtypeCapability {
  feature: string;
  status: CapabilityStatus;
  notes: string | null;
}

const NUMERIC_ZERO_ARG_BUILTINS = new Set([
  "acos",
  "acosh",
  "asin",
  "asinh",
  "atan",
  "atanh",
  "cbrt",
  "ceil",
  "cos",
  "cosh",
  "erf",
  "erfc",
  "exp",
  "exp10",
  "exp2",
  "expm1",
  "fabs",
  "floor",
  "ilogb",
  "infinite",
  "j0",
  "j1",
  "lgamma",
  "log",
  "log10",
  "log1p",
  "log2",
  "logb",
  "nan",
  "nearbyint",
  "rint",
  "round",
  "sin",
  "sinh",
  "sqrt",
  "tan",
  "tanh",
  "tgamma",
  "trunc",
  "y0",
  "y1",
]);

const NUMERIC_PAIR_ZERO_ARG_BUILTINS = new Set(["frexp", "lgamma_r", "modf"]);

const NUMERIC_PREDICATE_ZERO_ARG_BUILTINS = new Set([
  "isfinite",
  "isinfinite",
  "isnan",
  "isnormal",
  "signbit",
]);

const NUMERIC_TWO_ARG_BUILTINS = new Set([
  "atan2",
  "copysign",
  "fdim",
  "fmax",
  "fmin",
  "fmod",
  "hypot",
  "jn",
  "ldexp",
  "nextafter",
  "nexttoward",
  "pow",
  "remainder",
  "scalb",
  "scalbln",
  "scalbn",
  "yn",
]);

const NUMERIC_THREE_ARG_BUILTINS = new Set(["fma"]);

export const JQTYPE_CAPABILITIES: JqtypeCapability[] = [
  { feature: "object constructors", status: "supported", notes: null },
  { feature: "array constructors", status: "supported", notes: null },
  { feature: "field/index access", status: "supported", notes: "Dynamic indexes are conservative." },
  { feature: "map", status: "supported", notes: "Arrays and object values are modeled." },
  { feature: "select", status: "supported", notes: "Common predicates refine object shapes." },
  { feature: "if/elif/else", status: "supported", notes: "Branch outputs are joined conservatively." },
  { feature: "alternative operator //", status: "supported", notes: null },
  { feature: "length", status: "supported", notes: null },
  { feature: "type", status: "supported", notes: null },
  { feature: "tonumber", status: "supported", notes: null },
  { feature: "tostring", status: "supported", notes: null },
  { feature: "flatten", status: "supported", notes: "Dynamic flatten depths are widened conservatively." },
  { feature: "range", status: "supported", notes: null },
  { feature: "numeric math builtins", status: "supported", notes: "Common jq/libm transforms, predicates, constants, and 2/3-argument numeric functions are modeled." },
  { feature: "object merge via +", status: "supported", notes: null },
  { feature: "identity-root updates", status: "partial", notes: "Literal fields, indexes, iterators, and string-like dynamic keys are modeled." },
  { feature: "external variables", status: "supported", notes: "Provide AnalyzeOptions.externalVars." },
  { feature: "modules", status: "unsupported", notes: "import/include not supported." },
  { feature: "format strings", status: "supported", notes: "@json/@csv/@uri etc. return string." },
  { feature: "recursive descent", status: "supported", notes: "`..` returns input plus all descendants as a ZeroOrMore stream." },
  { feature: "labels/break", status: "unsupported", notes: "Disabled to stay aligned with jaq (which panics on label/break)." },
  { feature: "destructuring patterns", status: "partial", notes: "Single `as $var` is supported; object/array patterns are disabled to stay aligned with jaq (which fails to parse them)." },
  { feature: "def", status: "partial", notes: "Non-recursive user-defined functions via substitution; recursion widens to unknown after depth cap." },
];

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

export function analyzeFilter(
  source: string,
  input: InputShape = InputShape.unknown(),
  options: AnalyzeOptions = AnalyzeOptions.default(),
): AnalyzeReport {
  return new JqTypeChecker().analyzeFilter(source, input, options);
}

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
    input: InputShape = InputShape.unknown(),
    options: AnalyzeOptions = AnalyzeOptions.default(),
  ): AnalyzeReport {
    const resolvedOptions = resolveAnalyzeOptions(options);
    const sourceName = resolvedOptions.source_name;
    const parsed = parseFilter(source);
    if (!parsed.ok) {
      return parseFailureReport(parsed.message, source, sourceName);
    }

    const debugAst =
      resolvedOptions.output_format === "Tree"
        ? JSON.stringify(parsed.ast, null, 2)
        : null;

    const analyzer = new Analyzer(resolvedOptions, source);
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
        Diagnostic.error(
          `failed to parse jq filter: ${message}`,
          parseErrorSpan(message, source),
        ),
        sourceName,
      ),
    ],
    unsupported_features: [],
    debug_ast: null,
  };
}

function resolveAnalyzeOptions(options: AnalyzeOptions): ResolvedAnalyzeOptions {
  return {
    mode: options.mode ?? "Permissive",
    source_name: options.source_name ?? null,
    output_format: options.output_format ?? "Compact",
    max_union_members: options.max_union_members ?? 32,
    max_recursion_depth: options.max_recursion_depth ?? 32,
    externalVars: options.externalVars ?? {},
  };
}

function parseErrorSpan(message: string, source: string): SourceSpan | null {
  const match = /\((\d+):(\d+)\)/.exec(message);
  if (!match) return source.length === 0 ? { start: 0, end: 0 } : { start: 0, end: source.length };
  const line = Number(match[1]);
  const column = Number(match[2]);
  if (!Number.isFinite(line) || !Number.isFinite(column)) return null;
  const start = offsetFromLineColumn(source, line, column);
  const end = start >= source.length ? start : start + 1;
  return { start, end };
}

function offsetFromLineColumn(source: string, line: number, column: number): number {
  let offset = 0;
  for (let currentLine = 1; currentLine < line && offset < source.length; currentLine += 1) {
    const nextNewline = source.indexOf("\n", offset);
    if (nextNewline < 0) return source.length;
    offset = nextNewline + 1;
  }
  return Math.min(source.length, offset + Math.max(0, column));
}

class Analyzer {
  options: ResolvedAnalyzeOptions;
  source: string;
  diagnostics: Diagnostic[] = [];
  unsupportedFeatures: UnsupportedFeature[] = [];
  env = new Map<string, JTypeT>();
  labels = new Set<string>();
  filterArgs = new Map<string, FilterArgBinding>();
  defs = new Map<string, DefEntry>();
  defCallDepth = new Map<string, number>();
  readonly maxDefDepth = 4;
  missingPathNull = false;

  constructor(options: ResolvedAnalyzeOptions, source: string) {
    this.options = options;
    this.source = source;
    for (const [name, ty] of Object.entries(options.externalVars)) {
      this.env.set(normalizeVarName(name), ty);
    }
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
        return StreamType.one(this.stringType(expr, input));
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
        return this.analyzeCall(expr.name, expr.args, input);
      case "try":
        return this.analyzeTry(expr, input);
      case "unary":
        return this.analyzeUnary(expr, input);
      case "var":
        {
          const name = normalizeVarName(expr.name);
          const bound = this.env.get(name);
          if (bound !== undefined) return StreamType.one(bound);
          return this.unsupported(
            `unbound jq variable: $${name}`,
            "Unknown",
            this.spanForToken(`$${name}`),
          );
        }
      case "varDeclaration":
        return this.analyzeVarDeclaration(expr, input);
      case "reduce":
        return this.analyzeReduce(expr, input);
      case "foreach":
        return this.analyzeForeach(expr, input);
      case "recursiveDescent":
        return StreamType.zeroOrMore(JType.recursiveDescent(input));
      case "def":
        return this.analyzeDef(expr, input);
      case "label":
      case "break":
        // jaq (the Rust parser/runtime) does not support label/break yet —
        // it panics in its AST converter. Keep TS aligned by reporting as
        // unsupported. The withLabel helper below stays around for the
        // future, in case jaq adds support.
        return this.unsupported(
          "labels and break are not supported yet",
          input,
          this.spanForToken(expr.type),
        );
      case "format":
        return StreamType.one(JType.string());
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

  withVar<T>(name: string, ty: JTypeT, f: () => T): T {
    const key = normalizeVarName(name);
    const hadPrevious = this.env.has(key);
    const previous = this.env.get(key);
    this.env.set(key, ty);
    const out = f();
    if (hadPrevious) this.env.set(key, previous!);
    else this.env.delete(key);
    return out;
  }

  analyzeDef(node: DefAst, input: JTypeT): StreamType {
    // Parser already encodes arity into the name (e.g. "foo/2")
    const key = node.name;
    const previous = this.defs.get(key);
    const entry: DefEntry = {
      args: node.args,
      body: node.body,
      capturedEnv: new Map(this.env),
      capturedFilterArgs: new Map(this.filterArgs),
      capturedDefs: new Map(this.defs),
    };
    this.defs.set(key, entry);
    entry.capturedDefs.set(key, entry);

    const result =
      node.next === undefined ? StreamType.one(input) : this.analyze(node.next, input);

    if (previous === undefined) this.defs.delete(key);
    else this.defs.set(key, previous);
    return result;
  }

  callDef(
    entry: DefEntry,
    actualArgs: ExpressionAst[],
    input: JTypeT,
  ): StreamType {
    const key = `${entry.args.length}`;
    const depth = this.defCallDepth.get(key) ?? 0;
    if (depth >= this.maxDefDepth) {
      return StreamType.one("Unknown");
    }
    this.defCallDepth.set(key, depth + 1);

    const savedEnv = this.env;
    const savedFilterArgs = this.filterArgs;
    const savedDefs = this.defs;
    const newEnv = new Map(entry.capturedEnv);
    const newFilterArgs = new Map(entry.capturedFilterArgs);
    const newDefs = new Map(entry.capturedDefs);

    for (let i = 0; i < entry.args.length; i += 1) {
      const formal = entry.args[i]!;
      const actual = actualArgs[i]!;
      if (formal.type === "varArg") {
        // Evaluate actual against current call-site input, using caller's scope
        const bound = this.analyze(actual, input).item;
        newEnv.set(normalizeVarName(formal.name), bound);
      } else {
        // Filter arg: capture AST + caller's scope
        newFilterArgs.set(formal.name, {
          expr: actual,
          env: new Map(this.env),
          filterArgs: new Map(this.filterArgs),
          defs: new Map(this.defs),
        });
      }
    }

    this.env = newEnv;
    this.filterArgs = newFilterArgs;
    this.defs = newDefs;
    const out = this.analyze(entry.body, input);
    this.env = savedEnv;
    this.filterArgs = savedFilterArgs;
    this.defs = savedDefs;

    this.defCallDepth.set(key, depth);
    return out;
  }

  invokeFilterArg(binding: FilterArgBinding, input: JTypeT): StreamType {
    const savedEnv = this.env;
    const savedFilterArgs = this.filterArgs;
    const savedDefs = this.defs;
    this.env = new Map(binding.env);
    this.filterArgs = new Map(binding.filterArgs);
    this.defs = new Map(binding.defs);
    const out = this.analyze(binding.expr, input);
    this.env = savedEnv;
    this.filterArgs = savedFilterArgs;
    this.defs = savedDefs;
    return out;
  }

  withLabel<T>(rawName: string, f: () => T): T {
    const name = normalizeVarName(rawName);
    const had = this.labels.has(name);
    this.labels.add(name);
    const out = f();
    if (!had) this.labels.delete(name);
    return out;
  }

  withFreshMissingPathScope<T>(f: () => T): T {
    const previous = this.missingPathNull;
    this.missingPathNull = false;
    const out = f();
    this.missingPathNull = previous;
    return out;
  }

  analyzeVarDeclaration(node: VarDeclarationAst, input: JTypeT): StreamType {
    const bound = this.analyze(node.expr, input);
    if (bound.card === "Zero") return StreamType.zero();

    let perItem: StreamType | null = null;
    for (const item of flattenUnion(bound.item)) {
      const branch = this.withSimpleDestructuring(node.destructuring, item, () =>
        this.analyze(node.next, input),
      );
      perItem = perItem === null ? branch : StreamType.joinAlternative(perItem, branch);
    }
    const composed = perItem ?? StreamType.zero();
    return StreamType.new(composed.item, Cardinality.compose(bound.card, composed.card));
  }

  withSimpleDestructuring<T>(
    destructuring: VarDeclarationAst["destructuring"],
    ty: JTypeT,
    f: () => T,
  ): T {
    if (destructuring.length === 0) return f();
    // jaq does not parse object/array destructuring patterns. Keep TS
    // aligned: only single `as $var` is supported. The collectDestructuring*
    // / withBindings helpers below stay around for the future.
    if (destructuring.length === 1 && destructuring[0]!.type === "var") {
      return this.withVar(destructuring[0]!.name, ty, f);
    }
    this.warnOrError("destructuring variable bindings are not supported precisely yet");
    return f();
  }

  withBindings<T>(bindings: Map<string, JTypeT>, f: () => T): T {
    const names = Array.from(bindings.keys());
    if (names.length === 0) return f();
    const head = names[0]!;
    return this.withVar(head, bindings.get(head)!, () => {
      const rest = new Map(bindings);
      rest.delete(head);
      return this.withBindings(rest, f);
    });
  }

  analyzeReduce(node: ReduceAst, input: JTypeT): StreamType {
    return this.analyzeFold(node.expr, node.var, node.init, node.update, undefined, input, false);
  }

  analyzeForeach(node: ForeachAst, input: JTypeT): StreamType {
    return this.analyzeFold(node.expr, node.var, node.init, node.update, node.extract, input, true);
  }

  analyzeFold(
    expr: ExpressionAst,
    varName: string,
    initExpr: ExpressionAst,
    updateExpr: ExpressionAst,
    extractExpr: ExpressionAst | undefined,
    input: JTypeT,
    emitsIntermediate: boolean,
  ): StreamType {
    const xs = this.analyze(expr, input);
    const init = this.analyze(initExpr, input);
    if (xs.card === "Zero") return init;

    let acc = init.item;
    let update: StreamType = StreamType.one(acc);
    for (let i = 0; i < 8; i += 1) {
      let perItem: StreamType | null = null;
      for (const item of flattenUnion(xs.item)) {
        const branch = this.withVar(varName, item, () => this.analyze(updateExpr, acc));
        perItem = perItem === null ? branch : StreamType.joinAlternative(perItem, branch);
      }
      update = perItem ?? StreamType.zero();
      const next = JType.union([acc, update.item]);
      if (JType.toCompactString(next) === JType.toCompactString(acc)) break;
      acc = next;
    }

    if (emitsIntermediate) {
      if (extractExpr !== undefined) {
        let extracted: StreamType | null = null;
        for (const item of flattenUnion(xs.item)) {
          const branch = this.withVar(varName, item, () => this.analyze(extractExpr, acc));
          extracted = extracted === null ? branch : StreamType.joinAlternative(extracted, branch);
        }
        const out = extracted ?? StreamType.zero();
        return StreamType.new(out.item, Cardinality.compose(xs.card, out.card));
      }
      return StreamType.new(acc, "ZeroOrMore");
    }

    const card = init.card === "One" && update.card === "One" ? "One" : "ZeroOrMore";
    return StreamType.new(acc, card);
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
          this.missingPathNull = false;
          return StreamType.one(
            prop.required ? prop.ty : JType.union([prop.ty, "Null"]),
          );
        }
        if (input.Object.additional !== null) {
          this.missingPathNull = false;
          return StreamType.one(JType.union([input.Object.additional, "Null"]));
        }
        if (optional) {
          this.missingPathNull = false;
        } else {
          this.missingPathNull = true;
          this.warnOrError(
            `property "${key}" is not present on ${JType.toCompactString(input)}`,
            this.spanForProperty(key),
          );
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
    if (input === "Unknown") {
      this.missingPathNull = false;
      return StreamType.one("Unknown");
    }

    if (optional) {
      this.missingPathNull = false;
      this.warnOrError(
        `optional field \`${key}\` skipped non-object input: ${JType.toCompactString(input)}`,
      );
      return StreamType.zero();
    }
    this.missingPathNull = false;
    this.warnOrError(
      `field \`${key}\` may be applied to non-object input: ${JType.toCompactString(input)}`,
    );
    return StreamType.one("Unknown");
  }

  accessIndex(input: JTypeT, optional: boolean): StreamType {
    if (typeof input === "object") {
      if ("Array" in input) {
        this.missingPathNull = false;
        return StreamType.one(JType.union([input.Array.items, "Null"]));
      }
      if ("String" in input) {
        this.missingPathNull = false;
        return StreamType.one(JType.union([JType.string(), "Null"]));
      }
      if ("Union" in input) {
        let out = StreamType.zero();
        for (const item of input.Union) {
          out = StreamType.join(out, this.accessIndex(item, optional));
        }
        return out;
      }
    }
    if (input === "Null" && this.missingPathNull) {
      this.missingPathNull = false;
      return StreamType.one("Unknown");
    }
    if (input === "Unknown") {
      this.missingPathNull = false;
      return StreamType.one("Unknown");
    }

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
        this.missingPathNull = false;
        return StreamType.one(JType.union([input.Array.items, "Null"]));
      }
      if ("Object" in input) {
        this.missingPathNull = false;
        const values: JTypeT[] = Object.values(input.Object.properties).map(
          (p) => p.ty,
        );
        if (input.Object.additional !== null) values.push(input.Object.additional);
        values.push("Null");
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
    if (input === "Null" && this.missingPathNull) {
      this.missingPathNull = false;
      return StreamType.one("Unknown");
    }
    if (input === "Unknown") {
      this.missingPathNull = false;
      return StreamType.one("Unknown");
    }

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
        this.missingPathNull = false;
        return StreamType.zeroOrMore(input.Array.items);
      }
      if ("Object" in input) {
        this.missingPathNull = false;
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
    if (input === "Null" && this.missingPathNull) {
      this.missingPathNull = false;
      return StreamType.zeroOrMore("Unknown");
    }
    if (input === "Unknown") {
      this.missingPathNull = false;
      return StreamType.zeroOrMore("Unknown");
    }

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
      if ("Array" in input) {
        this.missingPathNull = false;
        return StreamType.one(JType.array(input.Array.items));
      }
      if ("String" in input) {
        this.missingPathNull = false;
        return StreamType.one(JType.string());
      }
      if ("Union" in input) {
        return StreamType.one(
          JType.union(input.Union.map((item) => this.slice(item).item)),
        );
      }
    }
    if (input === "Null" && this.missingPathNull) {
      this.missingPathNull = false;
      return StreamType.one("Unknown");
    }
    if (input === "Unknown") {
      this.missingPathNull = false;
      return StreamType.one("Unknown");
    }
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
      const left = this.analyze(node.left, input);
      const right = this.analyze(node.right, input);
      return StreamType.new(
        mathType(op, left.item, right.item),
        Cardinality.compose(left.card, right.card),
      );
    }
    if (op === "//") {
      const left = this.analyze(node.left, input);
      const right = this.analyze(node.right, input);
      return StreamType.new(
        altType(left.item, right.item),
        Cardinality.alternative(left.card, right.card),
      );
    }
    if (
      op === "=" ||
      op === "|=" ||
      op === "+=" ||
      op === "-=" ||
      op === "*=" ||
      op === "/=" ||
      op === "%=" ||
      op === "//="
    ) {
      return this.analyzeAssignment(node.left, op, node.right, input);
    }
    return this.unsupported(
      `unsupported binary operator \`${op}\``,
      "Unknown",
      this.spanForToken(op),
    );
  }

  analyzeAssignment(
    leftExpr: ExpressionAst,
    op: string,
    rightExpr: ExpressionAst,
    input: JTypeT,
  ): StreamType {
    if (op === "=") {
      const rhs = this.analyze(rightExpr, input);
      return StreamType.new(this.writeFilterPath(input, leftExpr, rhs.item), rhs.card);
    }
    if (op === "|=") {
      const old = this.analyze(leftExpr, input);
      const rhs = this.analyze(rightExpr, old.item);
      return StreamType.new(
        this.writeFilterPath(input, leftExpr, rhs.item),
        Cardinality.compose(old.card, rhs.card),
      );
    }

    const old = this.analyze(leftExpr, input);
    const rhs = this.analyze(rightExpr, input);
    const mathOp = op.slice(0, -1);
    const value =
      mathOp === "//"
        ? altType(old.item, rhs.item)
        : mathType(mathOp, old.item, rhs.item);
    return StreamType.new(
      this.writeFilterPath(input, leftExpr, value),
      Cardinality.compose(old.card, rhs.card),
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

  analyzeCall(rawName: string, args: ExpressionAst[], input: JTypeT): StreamType {
    // Parser passes "name/arity" — keep it for def/filterArg lookup; strip for builtin matching
    const defEntry = this.defs.get(rawName);
    if (defEntry !== undefined) {
      return this.callDef(defEntry, args, input);
    }
    if (args.length === 0) {
      const filterArg = this.filterArgs.get(rawName);
      if (filterArg !== undefined) {
        return this.invokeFilterArg(filterArg, input);
      }
    }
    const name = filterIdent(rawName);
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
    if (name === "tostring" && args.length === 0) return StreamType.one(tostringType(input));
    if (name === "tonumber" && args.length === 0) return StreamType.one(tonumberType(input));
    if (args.length === 0 && NUMERIC_ZERO_ARG_BUILTINS.has(name)) {
      return StreamType.one(JType.number());
    }
    if (args.length === 0 && NUMERIC_PAIR_ZERO_ARG_BUILTINS.has(name)) {
      return StreamType.one(JType.array(JType.number()));
    }
    if (args.length === 0 && NUMERIC_PREDICATE_ZERO_ARG_BUILTINS.has(name)) {
      return StreamType.one(JType.bool());
    }
    if (name === "now" && args.length === 0) return StreamType.one(JType.number());
    if (name === "keys" && args.length === 0) return StreamType.one(this.keysType(input));
    if (name === "not" && args.length === 0) return StreamType.one(notType(input));
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
    if (name === "add" && args.length === 0) return StreamType.one(this.addType(input));
    if (name === "flatten" && args.length === 0) {
      return StreamType.one(this.flattenType(input, { kind: "full" }));
    }
    if (name === "flatten" && args.length === 1) {
      const depthType = this.analyze(args[0]!, input).item;
      return StreamType.one(this.flattenType(input, flattenDepthFromType(depthType)));
    }
    if (name === "range" && args.length >= 1 && args.length <= 3) {
      for (const arg of args) this.analyze(arg, input);
      return StreamType.zeroOrMore(JType.number());
    }
    if (args.length === 2 && NUMERIC_TWO_ARG_BUILTINS.has(name)) {
      for (const arg of args) this.analyze(arg, input);
      return StreamType.one(JType.number());
    }
    if (args.length === 3 && NUMERIC_THREE_ARG_BUILTINS.has(name)) {
      for (const arg of args) this.analyze(arg, input);
      return StreamType.one(JType.number());
    }
    if (name === "join" && args.length === 1) {
      this.analyze(args[0]!, input);
      return StreamType.one(JType.string());
    }
    if (name === "transpose" && args.length === 0) {
      return StreamType.one(this.transposeType(input));
    }
    if (name === "ascii_upcase" && args.length === 0) {
      return StreamType.one(asciiUpcaseType(input));
    }
    if (name === "values" && args.length === 0) return this.filterValues(input);
    if (name === "nulls" && args.length === 0) return this.filterKind(input, "null");
    if (name === "booleans" && args.length === 0) return this.filterKind(input, "boolean");
    if (name === "numbers" && args.length === 0) return this.filterKind(input, "number");
    if (name === "strings" && args.length === 0) return this.filterKind(input, "string");
    if (name === "arrays" && args.length === 0) return this.filterKind(input, "array");
    if (name === "objects" && args.length === 0) return this.filterKind(input, "object");
    if (name === "iterables" && args.length === 0) {
      return StreamType.zeroOrOne(
        JType.union([
          this.filterKind(input, "array").item,
          this.filterKind(input, "object").item,
        ]),
      );
    }
    if (name === "scalars" && args.length === 0) {
      return StreamType.zeroOrOne(
        JType.union([
          this.filterKind(input, "null").item,
          this.filterKind(input, "boolean").item,
          this.filterKind(input, "number").item,
          this.filterKind(input, "string").item,
        ]),
      );
    }
    if ((name === "paths" || name === "leaf_paths") && (args.length === 0 || (name === "paths" && args.length === 1))) {
      for (const arg of args) this.analyze(arg, input);
      return StreamType.zeroOrMore(JType.array(JType.union([JType.string(), JType.number()])));
    }
    if (name === "path" && args.length === 1) {
      this.analyze(args[0]!, input);
      return StreamType.one(JType.array(JType.union([JType.string(), JType.number()])));
    }
    if (name === "getpath" && args.length === 1) {
      this.analyze(args[0]!, input);
      return StreamType.one("Unknown");
    }
    if (name === "setpath" && args.length === 2) {
      this.analyze(args[0]!, input);
      const value = this.analyze(args[1]!, input).item;
      return StreamType.one(this.writeDynamicObjectKey(input, value));
    }
    if ((name === "del" || name === "delpaths") && args.length === 1) {
      this.analyze(args[0]!, input);
      return StreamType.one(delType(input));
    }
    if (name === "walk" && args.length === 1) {
      return StreamType.one(this.walkType(args[0]!, input));
    }
    if ((name === "sort" || name === "unique") && args.length === 0) {
      return StreamType.one(this.sameArrayType(input));
    }
    if ((name === "sort_by" || name === "unique_by") && args.length === 1) {
      this.analyze(args[0]!, input);
      return StreamType.one(this.sameArrayType(input));
    }
    if (name === "group_by" && args.length === 1) {
      this.analyze(args[0]!, input);
      return StreamType.one(this.groupByType(input));
    }
    if ((name === "min" || name === "max") && args.length === 0) {
      return StreamType.one(this.arrayItemOrNull(input));
    }
    if ((name === "min_by" || name === "max_by") && args.length === 1) {
      this.analyze(args[0]!, input);
      return StreamType.one(this.arrayItemOrNull(input));
    }
    if (name === "reverse" && args.length === 0) {
      return StreamType.one(this.sameArrayOrString(input));
    }
    if ((name === "any" || name === "all") && args.length === 0) {
      return StreamType.one(JType.bool());
    }
    if ((name === "any" || name === "all") && args.length === 1) {
      this.analyze(args[0]!, input);
      return StreamType.one(JType.bool());
    }
    if ((name === "any" || name === "all") && args.length === 2) {
      this.analyze(args[0]!, input);
      this.analyze(args[1]!, input);
      return StreamType.one(JType.bool());
    }
    if ((name === "contains" || name === "inside") && args.length === 1) {
      this.analyze(args[0]!, input);
      return StreamType.one(JType.bool());
    }
    if ((name === "startswith" || name === "endswith") && args.length === 1) {
      this.analyze(args[0]!, input);
      return StreamType.one(JType.bool());
    }
    if (name === "test" && (args.length === 1 || args.length === 2)) {
      for (const arg of args) this.analyze(arg, input);
      return StreamType.one(JType.bool());
    }
    if ((name === "ltrimstr" || name === "rtrimstr") && args.length === 1) {
      this.analyze(args[0]!, input);
      return StreamType.one(trimStrType(input));
    }
    if (name === "ascii_downcase" && args.length === 0) {
      return StreamType.one(asciiDowncaseType(input));
    }
    if (name === "tojson" && args.length <= 1) {
      for (const arg of args) this.analyze(arg, input);
      return StreamType.one(JType.string());
    }
    if (name === "fromjson" && args.length === 0) {
      return StreamType.one("Unknown");
    }
    if (name === "split" && (args.length === 1 || args.length === 2)) {
      for (const arg of args) this.analyze(arg, input);
      return StreamType.one(JType.array(JType.string()));
    }
    if (name === "splits" && (args.length === 1 || args.length === 2)) {
      for (const arg of args) this.analyze(arg, input);
      return StreamType.zeroOrMore(JType.string());
    }
    if (name === "explode" && args.length === 0) {
      return StreamType.one(JType.array(JType.number()));
    }
    if (name === "implode" && args.length === 0) {
      return StreamType.one(JType.string());
    }
    if (name === "ascii" && args.length === 0) {
      return StreamType.one(JType.string());
    }
    if (name === "env" && args.length === 0) {
      return StreamType.one(JType.object({}, JType.string()));
    }
    if ((name === "$__loc__" || name === "input_line_number") && args.length === 0) {
      return StreamType.one("Unknown");
    }
    if (name === "error" && args.length === 0) {
      return StreamType.zero();
    }
    if (name === "error" && args.length === 1) {
      this.analyze(args[0]!, input);
      return StreamType.zero();
    }
    if (name === "debug" && args.length === 0) {
      return StreamType.one(input);
    }
    if (name === "debug" && args.length === 1) {
      this.analyze(args[0]!, input);
      return StreamType.one(input);
    }
    if (name === "stderr" && args.length === 0) {
      return StreamType.one(input);
    }
    if (name === "to_entries" && args.length === 0) {
      return StreamType.one(this.toEntriesType(input));
    }
    if (name === "from_entries" && args.length === 0) {
      return StreamType.one(this.fromEntriesType(input));
    }
    if (name === "with_entries" && args.length === 1) {
      return StreamType.one(this.withEntriesType(args[0]!, input));
    }
    if (name === "match" && (args.length === 1 || args.length === 2)) {
      for (const arg of args) this.analyze(arg, input);
      return StreamType.zeroOrMore(regexMatchType());
    }
    if (name === "capture" && (args.length === 1 || args.length === 2)) {
      for (const arg of args) this.analyze(arg, input);
      return StreamType.zeroOrMore(
        JType.object({}, JType.union([JType.string(), "Null"])),
      );
    }
    if (name === "scan" && (args.length === 1 || args.length === 2)) {
      for (const arg of args) this.analyze(arg, input);
      return StreamType.zeroOrMore(
        JType.union([JType.string(), JType.array(JType.string())]),
      );
    }
    if ((name === "gsub" || name === "sub") && (args.length === 2 || args.length === 3)) {
      for (const arg of args) this.analyze(arg, input);
      return StreamType.one(JType.string());
    }
    if (name === "limit" && args.length === 2) {
      this.analyze(args[0]!, input);
      const stream = this.analyze(args[1]!, input);
      return StreamType.zeroOrMore(stream.item);
    }
    if ((name === "first" || name === "last") && args.length === 1) {
      const stream = this.analyze(args[0]!, input);
      return StreamType.zeroOrOne(stream.item);
    }
    if (name === "nth" && args.length === 2) {
      this.analyze(args[0]!, input);
      const stream = this.analyze(args[1]!, input);
      return StreamType.zeroOrOne(stream.item);
    }
    if (name === "isempty" && args.length === 1) {
      this.analyze(args[0]!, input);
      return StreamType.one(JType.bool());
    }
    return this.unsupported(
      `unsupported builtin or call \`${name}\``,
      "Unknown",
      this.spanForToken(name),
    );
  }

  sameArrayType(input: JTypeT): JTypeT {
    if (typeof input === "object") {
      if ("Array" in input) return input;
      if ("Union" in input) return JType.union(input.Union.map((item) => this.sameArrayType(item)));
    }
    if (input === "Null" && this.missingPathNull) {
      this.missingPathNull = false;
      return "Unknown";
    }
    if (input === "Unknown") return JType.array("Unknown");
    this.warnOrError(`expected array input, got: ${JType.toCompactString(input)}`);
    return "Unknown";
  }

  sameArrayOrString(input: JTypeT): JTypeT {
    if (typeof input === "object") {
      if ("Array" in input || "String" in input) return input;
      if ("Union" in input) {
        return JType.union(input.Union.map((item) => this.sameArrayOrString(item)));
      }
    }
    if (input === "Unknown") return "Unknown";
    this.warnOrError(`expected array or string input, got: ${JType.toCompactString(input)}`);
    return "Unknown";
  }

  arrayItemOrNull(input: JTypeT): JTypeT {
    if (typeof input === "object") {
      if ("Array" in input) return JType.union([input.Array.items, "Null"]);
      if ("Union" in input) {
        return JType.union(input.Union.map((item) => this.arrayItemOrNull(item)));
      }
    }
    if (input === "Unknown") return "Unknown";
    if (input === "Null") return "Null";
    this.warnOrError(`expected array input, got: ${JType.toCompactString(input)}`);
    return "Unknown";
  }

  groupByType(input: JTypeT): JTypeT {
    if (typeof input === "object") {
      if ("Array" in input) return JType.array(JType.array(input.Array.items));
      if ("Union" in input) {
        return JType.union(input.Union.map((item) => this.groupByType(item)));
      }
    }
    if (input === "Unknown") return JType.array(JType.array("Unknown"));
    this.warnOrError(`group_by expected array input, got: ${JType.toCompactString(input)}`);
    return "Unknown";
  }

  walkType(f: ExpressionAst, input: JTypeT): JTypeT {
    const descended = JType.recursiveDescent(input);
    const mapped = this.analyze(f, descended).item;
    return JType.union([input, mapped]);
  }

  toEntriesType(input: JTypeT): JTypeT {
    let valueTy: JTypeT;
    if (typeof input === "object" && "Object" in input) {
      const values: JTypeT[] = Object.values(input.Object.properties).map((p) => p.ty);
      if (input.Object.additional !== null) values.push(input.Object.additional);
      valueTy = JType.union(values);
    } else if (input === "Unknown") {
      valueTy = "Unknown";
    } else if (typeof input === "object" && "Union" in input) {
      return JType.union(input.Union.map((item) => this.toEntriesType(item)));
    } else {
      this.warnOrError(`to_entries expected object input, got: ${JType.toCompactString(input)}`);
      return "Unknown";
    }
    return JType.array(
      JType.closedObject({
        key: { ty: JType.string(), required: true },
        value: { ty: valueTy, required: true },
      }),
    );
  }

  fromEntriesType(input: JTypeT): JTypeT {
    if (typeof input === "object") {
      if ("Array" in input) {
        const item = input.Array.items;
        if (typeof item === "object" && "Object" in item) {
          const valueTy =
            item.Object.properties["value"]?.ty ??
            item.Object.properties["v"]?.ty ??
            "Unknown";
          return JType.object({}, valueTy);
        }
        return JType.object({}, "Unknown");
      }
      if ("Union" in input) {
        return JType.union(input.Union.map((item) => this.fromEntriesType(item)));
      }
    }
    if (input === "Unknown") return JType.object({}, "Unknown");
    this.warnOrError(`from_entries expected array input, got: ${JType.toCompactString(input)}`);
    return "Unknown";
  }

  withEntriesType(f: ExpressionAst, input: JTypeT): JTypeT {
    const entries = this.toEntriesType(input);
    let mapped: JTypeT;
    if (typeof entries === "object" && "Array" in entries) {
      mapped = this.analyze(f, entries.Array.items).item;
    } else {
      mapped = this.analyze(f, entries).item;
    }
    return this.fromEntriesType(JType.array(mapped));
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
        this.missingPathNull = false;
        const keys: JTypeT[] = Object.keys(input.Object.properties).map((k) =>
          JType.stringLit(k),
        );
        if (input.Object.additional !== null) keys.push(JType.string());
        return JType.array(JType.union(keys));
      }
      if ("Array" in input) {
        this.missingPathNull = false;
        return JType.array(JType.number());
      }
      if ("Union" in input) {
        return JType.union(input.Union.map((item) => this.keysType(item)));
      }
    }
    if (input === "Null" && this.missingPathNull) {
      this.missingPathNull = false;
      return "Unknown";
    }
    if (input === "Unknown") {
      this.missingPathNull = false;
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
        const mapped = this.withFreshMissingPathScope(() =>
          this.analyze(mapper, input.Array.items),
        );
        this.missingPathNull = false;
        return StreamType.one(JType.array(mapped.item));
      }
      if ("Object" in input) {
        const values: JTypeT[] = Object.values(input.Object.properties).map(
          (p) => p.ty,
        );
        if (input.Object.additional !== null) values.push(input.Object.additional);
        const mapped = this.withFreshMissingPathScope(() =>
          this.analyze(mapper, JType.union(values)),
        );
        this.missingPathNull = false;
        return StreamType.one(JType.array(mapped.item));
      }
      if ("Union" in input) {
        const mapped = JType.union(
          input.Union.map((item) => this.mapCall(mapper, item).item),
        );
        this.missingPathNull = false;
        return StreamType.one(mapped);
      }
    }
    if (input === "Unknown") {
      this.missingPathNull = false;
      return StreamType.one(JType.array("Unknown"));
    }
    if (input === "Null" && this.missingPathNull) {
      this.missingPathNull = false;
      return StreamType.one("Unknown");
    }
    this.missingPathNull = false;
    this.warnOrError(
      `map may be applied to non-array/non-object input: ${JType.toCompactString(input)}`,
    );
    return StreamType.one("Unknown");
  }

  addType(input: JTypeT): JTypeT {
    if (typeof input === "object") {
      if ("Array" in input) {
        this.missingPathNull = false;
        const item = input.Array.items;
        if (item === "Never") return "Null";
        return JType.union([mathType("+", item, item), "Null"]);
      }
      if ("Union" in input) return JType.union(input.Union.map((item) => this.addType(item)));
    }
    if (input === "Null" && this.missingPathNull) {
      this.missingPathNull = false;
      return "Unknown";
    }
    if (input === "Unknown") {
      this.missingPathNull = false;
      return "Unknown";
    }
    this.warnOrError(
      `add may be applied to non-array input: ${JType.toCompactString(input)}`,
    );
    return "Unknown";
  }

  flattenType(input: JTypeT, depth: FlattenDepth): JTypeT {
    if (typeof input === "object") {
      if ("Array" in input) {
        this.missingPathNull = false;
        return JType.array(flattenItem(input.Array.items, depth));
      }
      if ("Union" in input) {
        return JType.union(input.Union.map((item) => this.flattenType(item, depth)));
      }
    }
    if (input === "Null" && this.missingPathNull) {
      this.missingPathNull = false;
      return "Unknown";
    }
    if (input === "Unknown") {
      this.missingPathNull = false;
      return JType.array("Unknown");
    }
    this.warnOrError(
      `flatten may be applied to non-array input: ${JType.toCompactString(input)}`,
    );
    return "Unknown";
  }

  transposeType(input: JTypeT): JTypeT {
    if (typeof input === "object") {
      if ("Array" in input) {
        this.missingPathNull = false;
        const item = input.Array.items;
        if (typeof item === "object" && "Array" in item) {
          return JType.array(JType.array(item.Array.items));
        }
        if (item === "Unknown") return JType.array(JType.array("Unknown"));
        this.warnOrError(
          `transpose may be applied to non-array items: ${JType.toCompactString(item)}`,
        );
        return JType.array(JType.array("Unknown"));
      }
      if ("Union" in input) {
        return JType.union(input.Union.map((item) => this.transposeType(item)));
      }
    }
    if (input === "Null" && this.missingPathNull) {
      this.missingPathNull = false;
      return "Unknown";
    }
    if (input === "Unknown") {
      this.missingPathNull = false;
      return JType.array(JType.array("Unknown"));
    }
    this.warnOrError(
      `transpose may be applied to non-array input: ${JType.toCompactString(input)}`,
    );
    return "Unknown";
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

  stringType(value: StrAst, input: JTypeT): JTypeT {
    const lit = literalString(value);
    if (lit !== null) return JType.stringLit(lit);
    if (value.interpolated === true) {
      for (const part of value.parts) {
        if (typeof part !== "string") this.analyze(part, input);
      }
    }
    return JType.string();
  }

  writeFilterPath(input: JTypeT, target: ExpressionAst, value: JTypeT): JTypeT {
    switch (target.type) {
      case "identity":
        return value;
      case "index": {
        const base = this.analyze(target.expr, input).item;
        const updated = this.writeIndex(base, target.index, value);
        return this.writeFilterPath(input, target.expr, updated);
      }
      case "iterator": {
        const base = this.analyze(target.expr, input).item;
        const updated = this.writeArrayIndex(base, value);
        return this.writeFilterPath(input, target.expr, updated);
      }
      case "slice": {
        if (target.from !== undefined) this.analyze(target.from, input);
        if (target.to !== undefined) this.analyze(target.to, input);
        const base = this.analyze(target.expr, input).item;
        const updated = this.writeSlice(base, value);
        return this.writeFilterPath(input, target.expr, updated);
      }
      default:
        this.warnOrError("assignment left-hand side is not a supported identity-root path");
        return "Unknown";
    }
  }

  writeSlice(input: JTypeT, value: JTypeT): JTypeT {
    const rhsItem =
      typeof value === "object" && "Array" in value ? value.Array.items : "Unknown";
    if (typeof input === "object") {
      if ("Array" in input) {
        return JType.array(JType.union([input.Array.items, rhsItem]));
      }
      if ("Union" in input) {
        return JType.union(input.Union.map((item) => this.writeSlice(item, value)));
      }
    }
    if (input === "Null" || input === "Unknown") return JType.array(rhsItem);
    this.warnOrError(
      `slice assignment may be applied to non-array input: ${JType.toCompactString(input)}`,
    );
    return "Unknown";
  }

  writeIndex(
    input: JTypeT,
    index: string | ExpressionAst,
    value: JTypeT,
  ): JTypeT {
    if (typeof index === "string") return this.writeField(input, index, value);
    if (index.type === "num") return this.writeArrayIndex(input, value);
    if (index.type === "str" && index.interpolated === false) {
      return this.writeField(input, index.value, value);
    }
    const keyType = this.analyze(index, input).item;
    return this.writeDynamicIndex(input, keyType, value);
  }

  writeField(input: JTypeT, key: string, value: JTypeT): JTypeT {
    if (typeof input === "object") {
      if ("Object" in input) {
        return JType.object(
          {
            ...input.Object.properties,
            [key]: { ty: value, required: true },
          },
          input.Object.additional,
        );
      }
      if ("Union" in input) {
        return JType.union(input.Union.map((item) => this.writeField(item, key, value)));
      }
    }
    if (input === "Null") {
      return JType.closedObject({ [key]: { ty: value, required: true } });
    }
    if (input === "Unknown") {
      return JType.openObject({ [key]: { ty: value, required: true } });
    }
    this.warnOrError(
      `field assignment may be applied to non-object input: ${JType.toCompactString(input)}`,
    );
    return "Unknown";
  }

  writeArrayIndex(input: JTypeT, value: JTypeT): JTypeT {
    if (typeof input === "object") {
      if ("Array" in input) return JType.array(value);
      if ("Union" in input) {
        return JType.union(input.Union.map((item) => this.writeArrayIndex(item, value)));
      }
    }
    if (input === "Null" || input === "Unknown") return JType.array(value);
    this.warnOrError(
      `array assignment may be applied to non-array input: ${JType.toCompactString(input)}`,
    );
    return "Unknown";
  }

  writeDynamicIndex(input: JTypeT, keyType: JTypeT, value: JTypeT): JTypeT {
    const keys = finiteStringLiterals(keyType);
    if (keys !== null) {
      let out = input;
      for (const key of keys) out = this.writeField(out, key, value);
      return out;
    }
    if (isStringLike(keyType)) return this.writeDynamicObjectKey(input, value);
    if (isNumberLike(keyType)) return this.writeArrayIndex(input, value);

    if (typeof input === "object" && "Union" in input) {
      return JType.union(
        input.Union.map((item) => this.writeDynamicIndex(item, keyType, value)),
      );
    }
    if (input === "Unknown") return "Unknown";
    this.warnOrError(
      `dynamic assignment key may be non-string/non-number: ${JType.toCompactString(keyType)}`,
    );
    return JType.union([this.writeDynamicObjectKey(input, value), "Unknown"]);
  }

  writeDynamicObjectKey(input: JTypeT, value: JTypeT): JTypeT {
    if (typeof input === "object") {
      if ("Object" in input) {
        const props: Record<string, Property> = {};
        for (const [key, prop] of Object.entries(input.Object.properties)) {
          props[key] = { ...prop, ty: JType.union([prop.ty, value]) };
        }
        const additional =
          input.Object.additional === null
            ? value
            : JType.union([input.Object.additional, value]);
        return JType.object(props, additional);
      }
      if ("Union" in input) {
        return JType.union(
          input.Union.map((item) => this.writeDynamicObjectKey(item, value)),
        );
      }
    }
    if (input === "Null" || input === "Unknown") return JType.object({}, value);
    this.warnOrError(
      `dynamic object assignment may be applied to non-object input: ${JType.toCompactString(input)}`,
    );
    return "Unknown";
  }

  unsupported(
    feature: string,
    fallback: JTypeT,
    span: SourceSpan | null = null,
  ): StreamType {
    this.unsupportedFeatures.push({ feature, span });
    this.warnOrError(feature, span);
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

  spanForToken(token: string): SourceSpan | null {
    if (token.length === 0) return null;
    const start = this.source.indexOf(token);
    if (start < 0) return null;
    return { start, end: start + token.length };
  }

  spanForProperty(key: string): SourceSpan | null {
    const dotted = `.${key}`;
    const dottedSpan = this.spanForToken(dotted);
    if (dottedSpan !== null) return dottedSpan;
    return this.spanForToken(key);
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

function mathType(op: string, left: JTypeT, right: JTypeT): JTypeT {
  if (op === "+") return plusType(left, right);
  return numericMathType(left, right);
}

function numericMathType(left: JTypeT, right: JTypeT): JTypeT {
  if (typeof left === "object" && "Union" in left) {
    return JType.union(left.Union.map((item) => numericMathType(item, right)));
  }
  if (typeof right === "object" && "Union" in right) {
    return JType.union(right.Union.map((item) => numericMathType(left, item)));
  }
  if (left === "Unknown" || right === "Unknown") return "Unknown";
  if (left === "Null" && right === "Null") return JType.number();
  if (left === "Null" && typeof right === "object" && "Number" in right) return JType.number();
  if (right === "Null" && typeof left === "object" && "Number" in left) return JType.number();
  if (
    typeof left === "object" &&
    "Number" in left &&
    typeof right === "object" &&
    "Number" in right
  ) {
    return JType.number();
  }
  return "Unknown";
}

function plusType(left: JTypeT, right: JTypeT): JTypeT {
  if (typeof left === "object" && "Union" in left) {
    return JType.union(left.Union.map((item) => plusType(item, right)));
  }
  if (typeof right === "object" && "Union" in right) {
    return JType.union(right.Union.map((item) => plusType(left, item)));
  }
  if (left === "Unknown" || right === "Unknown") return "Unknown";
  if (left === "Null") return right;
  if (right === "Null") return left;
  if (
    typeof left === "object" &&
    "Number" in left &&
    typeof right === "object" &&
    "Number" in right
  ) {
    return JType.number();
  }
  if (
    typeof left === "object" &&
    "String" in left &&
    typeof right === "object" &&
    "String" in right
  ) {
    if (left.String !== "Any" && right.String !== "Any") {
      return JType.stringLit(`${left.String.Literal}${right.String.Literal}`);
    }
    return JType.string();
  }
  if (
    typeof left === "object" &&
    "Array" in left &&
    typeof right === "object" &&
    "Array" in right
  ) {
    return JType.array(JType.union([left.Array.items, right.Array.items]));
  }
  if (
    typeof left === "object" &&
    "Object" in left &&
    typeof right === "object" &&
    "Object" in right
  ) {
    return JType.object(
      mergeObjectProperties(left.Object, right.Object),
      mergeAdditional(left.Object.additional, right.Object.additional),
    );
  }
  return "Unknown";
}

function mergeObjectProperties(left: ObjectType, right: ObjectType): Record<string, Property> {
  const props: Record<string, Property> = {};
  for (const [key, prop] of Object.entries(left.properties)) {
    props[key] =
      right.additional === null ? { ...prop } : { ...prop, ty: JType.union([prop.ty, right.additional]) };
  }
  for (const [key, prop] of Object.entries(right.properties)) {
    props[key] = { ...prop };
  }
  return props;
}

function mergeAdditional(left: JTypeT | null, right: JTypeT | null): JTypeT | null {
  if (left !== null && right !== null) return JType.union([left, right]);
  return left ?? right;
}

function altType(left: JTypeT, right: JTypeT): JTypeT {
  if (left === "Unknown") return "Unknown";
  const kept = withoutNullFalse(left);
  if (kept === "Never") return right;
  if (JType.toCompactString(kept) === JType.toCompactString(left)) return left;
  return JType.union([kept, right]);
}

function withoutNullFalse(input: JTypeT): JTypeT {
  if (input === "Null") return "Never";
  if (typeof input === "object") {
    if ("Bool" in input) {
      if (input.Bool === "Any") return JType.boolLit(true);
      return input.Bool.Literal ? JType.boolLit(true) : "Never";
    }
    if ("Union" in input) return JType.union(input.Union.map(withoutNullFalse));
  }
  return input;
}

function tostringType(input: JTypeT): JTypeT {
  if (typeof input === "object") {
    if ("Union" in input) return JType.union(input.Union.map(tostringType));
    if ("String" in input) return input;
    if ("Number" in input) {
      return input.Number === "Any" ? JType.string() : JType.stringLit(input.Number.Literal);
    }
    if ("Bool" in input) {
      if (input.Bool === "Any") {
        return JType.union([JType.stringLit("true"), JType.stringLit("false")]);
      }
      return JType.stringLit(String(input.Bool.Literal));
    }
  }
  if (input === "Null") return JType.stringLit("null");
  if (input === "Never") return "Never";
  return JType.string();
}

function tonumberType(input: JTypeT): JTypeT {
  if (typeof input === "object") {
    if ("Union" in input) return JType.union(input.Union.map(tonumberType));
    if ("Number" in input) return input;
    if ("String" in input) {
      if (input.String !== "Any" && Number.isFinite(Number(input.String.Literal))) {
        return JType.numberLit(input.String.Literal);
      }
      return JType.number();
    }
  }
  if (input === "Never") return "Never";
  return JType.number();
}

function notType(input: JTypeT): JTypeT {
  const truthy = JType.isTruthyLiteral(input);
  return truthy === null ? JType.bool() : JType.boolLit(!truthy);
}

function delType(input: JTypeT): JTypeT {
  if (typeof input === "object") {
    if ("Object" in input) {
      const props: Record<string, Property> = {};
      for (const [key, prop] of Object.entries(input.Object.properties)) {
        props[key] = { ...prop, required: false };
      }
      return JType.object(
        props,
        input.Object.additional ?? "Unknown",
      );
    }
    if ("Array" in input) return JType.array(input.Array.items);
    if ("Union" in input) return JType.union(input.Union.map(delType));
  }
  if (input === "Unknown") return "Unknown";
  return input;
}

function trimStrType(input: JTypeT): JTypeT {
  if (typeof input === "object") {
    if ("String" in input) return JType.string();
    if ("Union" in input) return JType.union(input.Union.map(trimStrType));
  }
  if (input === "Unknown") return "Unknown";
  return input;
}

function asciiDowncaseType(input: JTypeT): JTypeT {
  if (typeof input === "object") {
    if ("Union" in input) return JType.union(input.Union.map(asciiDowncaseType));
    if ("String" in input && input.String !== "Any") {
      return JType.stringLit(input.String.Literal.toLowerCase());
    }
  }
  if (input === "Never") return "Never";
  return JType.string();
}

function regexMatchType(): JTypeT {
  const captureObj = JType.closedObject({
    offset: { ty: JType.number(), required: true },
    length: { ty: JType.number(), required: true },
    string: { ty: JType.union([JType.string(), "Null"]), required: true },
    name: { ty: JType.union([JType.string(), "Null"]), required: true },
  });
  return JType.closedObject({
    offset: { ty: JType.number(), required: true },
    length: { ty: JType.number(), required: true },
    string: { ty: JType.string(), required: true },
    captures: { ty: JType.array(captureObj), required: true },
  });
}

function asciiUpcaseType(input: JTypeT): JTypeT {
  if (typeof input === "object") {
    if ("Union" in input) return JType.union(input.Union.map(asciiUpcaseType));
    if ("String" in input && input.String !== "Any") {
      return JType.stringLit(input.String.Literal.toUpperCase());
    }
  }
  if (input === "Never") return "Never";
  return JType.string();
}

function finiteStringLiterals(input: JTypeT): string[] | null {
  if (typeof input === "object") {
    if ("String" in input && input.String !== "Any") return [input.String.Literal];
    if ("Union" in input) {
      const out: string[] = [];
      for (const item of input.Union) {
        const literals = finiteStringLiterals(item);
        if (literals === null) return null;
        out.push(...literals);
      }
      return out;
    }
  }
  return null;
}

function isStringLike(input: JTypeT): boolean {
  if (typeof input === "object") {
    if ("String" in input) return true;
    if ("Union" in input) return input.Union.every(isStringLike);
  }
  return false;
}

function isNumberLike(input: JTypeT): boolean {
  if (typeof input === "object") {
    if ("Number" in input) return true;
    if ("Union" in input) return input.Union.every(isNumberLike);
  }
  return false;
}

function normalizeVarName(name: string): string {
  return name.startsWith("$") ? name.slice(1) : name;
}

// Kept around for the future: full object/array destructuring support is
// currently disabled in `withSimpleDestructuring` to stay aligned with jaq
// (which fails to parse these patterns). Re-enable both sides together if
// jaq adds support.
// eslint-disable-next-line @typescript-eslint/no-unused-vars
function collectDestructuringBindings(
  pattern: DestructuringAst,
  ty: JTypeT,
): Map<string, JTypeT> {
  const out = new Map<string, JTypeT>();
  walkDestructuring(pattern, ty, out);
  return out;
}

function walkDestructuring(
  pattern: DestructuringAst,
  ty: JTypeT,
  out: Map<string, JTypeT>,
): void {
  if (pattern.type === "var") {
    const name = normalizeVarName(pattern.name);
    out.set(name, ty);
    return;
  }
  if (pattern.type === "arrayDestructuring") {
    walkArrayDestructuring(pattern, ty, out);
    return;
  }
  if (pattern.type === "objectDestructuring") {
    walkObjectDestructuring(pattern, ty, out);
  }
}

function walkArrayDestructuring(
  pattern: ArrayDestructuringAst,
  ty: JTypeT,
  out: Map<string, JTypeT>,
): void {
  const itemTy = arrayItemFor(ty);
  for (const child of pattern.destructuring) {
    walkDestructuring(child, itemTy, out);
  }
}

function arrayItemFor(ty: JTypeT): JTypeT {
  if (ty === "Unknown") return "Unknown";
  if (typeof ty === "object") {
    if ("Array" in ty) return JType.union([ty.Array.items, "Null"]);
    if ("Union" in ty) {
      return JType.union(ty.Union.map(arrayItemFor));
    }
  }
  return "Null";
}

function walkObjectDestructuring(
  pattern: ObjectDestructuringAst,
  ty: JTypeT,
  out: Map<string, JTypeT>,
): void {
  for (const entry of pattern.entries) {
    const keyLit = objectDestructuringKey(entry.key);
    if (entry.destructuring === undefined) {
      const name = normalizeVarName((entry.key as { name: string }).name);
      const valueTy = keyLit !== null ? objectFieldFor(ty, keyLit) : "Unknown";
      out.set(name, valueTy);
      continue;
    }
    const valueTy = keyLit !== null ? objectFieldFor(ty, keyLit) : "Unknown";
    walkDestructuring(entry.destructuring, valueTy, out);
  }
}

function objectDestructuringKey(
  key: { type: "var"; name: string } | ExpressionAst | string,
): string | null {
  if (typeof key === "string") return key;
  if ("type" in key) {
    if (key.type === "str" && key.interpolated === false) return key.value;
    if (key.type === "var") return normalizeVarName(key.name);
  }
  return null;
}

function objectFieldFor(ty: JTypeT, key: string): JTypeT {
  if (ty === "Unknown") return "Unknown";
  if (typeof ty === "object") {
    if ("Object" in ty) {
      const prop = ty.Object.properties[key];
      if (prop) return prop.required ? prop.ty : JType.union([prop.ty, "Null"]);
      if (ty.Object.additional !== null) return JType.union([ty.Object.additional, "Null"]);
      return "Null";
    }
    if ("Union" in ty) {
      return JType.union(ty.Union.map((item) => objectFieldFor(item, key)));
    }
  }
  return "Null";
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

type FlattenDepth =
  | { kind: "full" }
  | { kind: "exact"; depth: number }
  | { kind: "unknown" };

function flattenDepthFromType(input: JTypeT): FlattenDepth {
  if (typeof input === "object" && "Number" in input && input.Number !== "Any") {
    const depth = Number(input.Number.Literal);
    if (Number.isInteger(depth) && depth >= 0) return { kind: "exact", depth };
  }
  return { kind: "unknown" };
}

function flattenItem(input: JTypeT, depth: FlattenDepth): JTypeT {
  if (depth.kind === "full") return flattenItemFull(input);
  if (depth.kind === "exact") return flattenItemExact(input, depth.depth);
  return flattenItemUnknownDepth(input);
}

function flattenItemFull(input: JTypeT): JTypeT {
  if (typeof input === "object") {
    if ("Array" in input) return flattenItemFull(input.Array.items);
    if ("Union" in input) return JType.union(input.Union.map(flattenItemFull));
  }
  return input;
}

function flattenItemExact(input: JTypeT, depth: number): JTypeT {
  if (depth === 0) return input;
  if (typeof input === "object") {
    if ("Array" in input) return flattenItemExact(input.Array.items, depth - 1);
    if ("Union" in input) {
      return JType.union(input.Union.map((item) => flattenItemExact(item, depth)));
    }
  }
  return input;
}

function flattenItemUnknownDepth(input: JTypeT): JTypeT {
  if (typeof input === "object") {
    if ("Array" in input) {
      const nested = input.Array.items;
      return JType.union([JType.array(nested), flattenItemUnknownDepth(nested)]);
    }
    if ("Union" in input) return JType.union(input.Union.map(flattenItemUnknownDepth));
  }
  return input;
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
