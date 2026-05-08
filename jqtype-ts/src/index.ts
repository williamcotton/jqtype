export {
  AnalyzeOptions,
  AnalyzeReport,
  InputShape,
  JqTypeChecker,
  type AnalysisMode,
  type OutputFormat,
  type UnsupportedFeature,
} from "./analyze.js";

export { Diagnostic, SourceSpan, type Severity } from "./diagnostic.js";

export {
  jsonSchemaToType,
  sampleToType,
  typeToJsonSchema,
  valueFitsType,
} from "./schema.js";

export { Cardinality, StreamType } from "./stream.js";

export {
  JType,
  jtypeKind,
  toCompactString,
  type ArrayType,
  type BoolType,
  type JTypeKind,
  type NumberType,
  type ObjectType,
  type Property,
  type StringType,
} from "./types.js";

export { parseFilter, type ParseResult } from "./parser.js";
