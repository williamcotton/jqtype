import { parse as jqParse, JqParseError } from "@jq-tools/jq";
import type { ExpressionAst, ProgAst } from "@jq-tools/jq";

export type {
  ProgAst,
  ExpressionAst,
  AtomAst,
  BinaryAst,
  BinaryOperator,
  UnaryAst,
  UnaryOperator,
  IdentityAst,
  NumAst,
  StrAst,
  SimpleStrAst,
  InterpolatedStrAst,
  BoolAst,
  NullAst,
  FilterAst,
  IfAst,
  TryAst,
  ReduceAst,
  ForeachAst,
  VarAst,
  VarDeclarationAst,
  IndexAst,
  SliceAst,
  IteratorAst,
  ArrayAst,
  ObjectAst,
  ObjectEntryAst,
  RecursiveDescentAst,
  FormatAst,
  LabelAst,
  BreakAst,
  DefAst,
  ArgAst,
  VarArgAst,
  FilterArgAst,
  DestructuringAst,
  ArrayDestructuringAst,
  ObjectDestructuringAst,
  ObjectDestructuringEntryAst,
} from "@jq-tools/jq";

export interface ParseSuccess {
  ok: true;
  ast: ProgAst;
}

export interface ParseFailure {
  ok: false;
  message: string;
}

export type ParseResult = ParseSuccess | ParseFailure;

export function parseFilter(source: string): ParseResult {
  try {
    const ast = jqParse(source);
    return { ok: true, ast };
  } catch (err) {
    if (err instanceof JqParseError) {
      return { ok: false, message: err.message };
    }
    if (err instanceof Error) {
      return { ok: false, message: err.message };
    }
    return { ok: false, message: String(err) };
  }
}

export function rootExpression(prog: ProgAst): ExpressionAst | undefined {
  return prog.expr;
}
