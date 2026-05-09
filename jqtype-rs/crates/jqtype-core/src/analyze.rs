use std::collections::BTreeMap;

use jaq_syn::filter::{AssignOp, BinaryOp, Filter as JaqFilter, FoldType, KeyVal};
use jaq_syn::path::{Opt, Part};
use jaq_syn::string;
use jaq_syn::{MathOp, OrdOp, Span, Spanned};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::diagnostic::{Diagnostic, Severity, SourceSpan};
use crate::schema::{json_schema_to_type, sample_to_type, type_to_json_schema};
use crate::stream::{Cardinality, StreamType};
use crate::types::{BoolType, JType, NumberType, Property, StringType};

type Filter = JaqFilter<String, String, String>;
type SpannedFilter = Spanned<Filter>;

/// Controls whether possibly-invalid operations widen to `Unknown` with a
/// warning ([`AnalysisMode::Permissive`]) or become hard errors that can
/// produce a non-zero exit code ([`AnalysisMode::Strict`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalysisMode {
    Permissive,
    Strict,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputFormat {
    Compact,
    JsonSchema,
    Tree,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnalyzeOptions {
    pub mode: AnalysisMode,
    pub source_name: Option<String>,
    pub output_format: OutputFormat,
    pub max_union_members: usize,
    pub max_recursion_depth: usize,
}

impl Default for AnalyzeOptions {
    fn default() -> Self {
        Self {
            mode: AnalysisMode::Permissive,
            source_name: None,
            output_format: OutputFormat::Compact,
            max_union_members: 32,
            max_recursion_depth: 32,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnsupportedFeature {
    pub feature: String,
    pub span: Option<SourceSpan>,
}

/// Full result of analyzing a jq filter against an [`InputShape`].
///
/// Every field is serializable so reports can be rendered as JSON or
/// shipped across process boundaries.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnalyzeReport {
    /// Inferred output stream type (item shape + cardinality).
    pub output: StreamType,
    /// Warnings or errors collected during analysis. Spans are byte
    /// offsets into the original filter string.
    pub diagnostics: Vec<Diagnostic>,
    /// Features the analyzer encountered but did not understand. The
    /// filter still produces a result (typically widened to `Unknown`)
    /// but callers may want to surface this list to users.
    pub unsupported_features: Vec<UnsupportedFeature>,
    /// Pretty-printed jaq AST when [`OutputFormat::Tree`] was requested.
    pub debug_ast: Option<String>,
}

impl AnalyzeReport {
    pub fn output_type(&self) -> &StreamType {
        &self.output
    }

    pub fn to_json_schema_value(&self) -> Value {
        json!({
            "stream": self.output.card.as_str(),
            "schema": type_to_json_schema(&self.output.item),
        })
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diag| matches!(diag.severity, Severity::Error))
    }
}

/// Description of the filter's input. The analyzer normalizes every
/// variant into a single [`JType`] before walking the AST.
///
/// - [`InputShape::Type`]: a host-supplied [`JType`] — the most precise
///   form, useful when the embedding application already has its own type
///   model.
/// - [`InputShape::JsonSchema`]: a JSON Schema value, converted via
///   [`crate::json_schema_to_type`].
/// - [`InputShape::Sample`]: a single concrete JSON value, converted via
///   [`crate::sample_to_type`] (literals are preserved).
/// - [`InputShape::Unknown`]: no information; the filter is analyzed
///   against `JType::Unknown`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum InputShape {
    Type(JType),
    JsonSchema(Value),
    Sample(Value),
    Unknown,
}

impl InputShape {
    pub fn from_type(ty: JType) -> Self {
        Self::Type(ty)
    }

    pub fn from_json_schema(schema: Value) -> Self {
        Self::JsonSchema(schema)
    }

    pub fn from_sample(sample: Value) -> Self {
        Self::Sample(sample)
    }

    pub fn into_type(self) -> JType {
        match self {
            InputShape::Type(ty) => ty,
            InputShape::JsonSchema(schema) => json_schema_to_type(&schema),
            InputShape::Sample(sample) => sample_to_type(&sample),
            InputShape::Unknown => JType::Unknown,
        }
    }
}

/// Entry point for analyzing jq filters. Holds parser/builtin state and
/// is cheap to construct via [`JqTypeChecker::new`].
#[derive(Default)]
pub struct JqTypeChecker;

impl JqTypeChecker {
    pub fn new() -> Self {
        Self
    }

    /// Parse `source` and return a debug-printed representation of the
    /// jaq AST. Useful for the `--debug-ast` CLI flag and for embedders
    /// who want to inspect parser output without the full analyzer
    /// pipeline.
    pub fn parse_debug_ast(&self, source: &str) -> Result<String, AnalyzeReport> {
        match parse_filter(source) {
            Ok(filter) => Ok(format!("{filter:#?}")),
            Err(report) => Err(report),
        }
    }

    /// Analyze `source` against `input` and return a full
    /// [`AnalyzeReport`]. Parser failures are returned as a report with a
    /// single error diagnostic rather than a panic; the analyzer never
    /// panics on syntactically valid input.
    pub fn analyze_filter(
        &self,
        source: &str,
        input: InputShape,
        options: AnalyzeOptions,
    ) -> AnalyzeReport {
        let source_name = options.source_name.clone();
        let filter = match parse_filter(source) {
            Ok(filter) => filter,
            Err(mut report) => {
                for diagnostic in &mut report.diagnostics {
                    diagnostic.source_name = source_name.clone();
                }
                return report;
            }
        };

        let debug_ast = if matches!(options.output_format, OutputFormat::Tree) {
            Some(format!("{filter:#?}"))
        } else {
            None
        };

        let mut analyzer = Analyzer::new(options);
        let output = analyzer.analyze(&filter.body, input.into_type());
        AnalyzeReport {
            output,
            diagnostics: analyzer.diagnostics,
            unsupported_features: analyzer.unsupported_features,
            debug_ast,
        }
    }
}

fn parse_filter(source: &str) -> Result<jaq_syn::Main<Filter>, AnalyzeReport> {
    let parsed = std::panic::catch_unwind(|| {
        jaq_syn::parse(source, |parser| parser.module(|parser| parser.term()))
            .map(|module| module.conv(source))
    })
    .ok()
    .flatten();

    parsed.ok_or_else(|| AnalyzeReport {
        output: StreamType::zero(),
        diagnostics: vec![Diagnostic::error(
            "failed to parse jq filter",
            Some(SourceSpan::new(0, source.len())),
        )],
        unsupported_features: vec![],
        debug_ast: None,
    })
}

struct Analyzer {
    options: AnalyzeOptions,
    diagnostics: Vec<Diagnostic>,
    unsupported_features: Vec<UnsupportedFeature>,
    env: BTreeMap<String, JType>,
    missing_path_null: bool,
}

#[derive(Clone, Debug)]
struct PredicateRefinement {
    when_true: JType,
    when_false: JType,
}

impl Analyzer {
    fn new(options: AnalyzeOptions) -> Self {
        Self {
            options,
            diagnostics: Vec::new(),
            unsupported_features: Vec::new(),
            env: BTreeMap::new(),
            missing_path_null: false,
        }
    }

    fn analyze(&mut self, filter: &SpannedFilter, input: JType) -> StreamType {
        let span = filter.1.clone();
        match &filter.0 {
            Filter::Id => StreamType::one(input),
            Filter::Num(value) => StreamType::one(JType::number_lit(value.clone())),
            Filter::Str(value) => StreamType::one(self.string_type(value, input)),
            Filter::Array(None) => StreamType::one(JType::array(JType::Never)),
            Filter::Array(Some(inner)) => {
                let inner = self.analyze(inner, input);
                StreamType::one(JType::array(inner.item))
            }
            Filter::Object(items) => self.analyze_object(items, input),
            Filter::Path(base, path) => self.analyze_path(base, path, input),
            Filter::Binary(left, op, right) => self.analyze_binary(left, op, right, input),
            Filter::Ite(branches, else_branch) => {
                self.analyze_if(branches, else_branch.as_deref(), input)
            }
            Filter::Call(name, args) => self.analyze_call(name, args, span, input),
            Filter::Try(inner) => self.analyze(inner, input),
            Filter::TryCatch(inner, catch) => {
                let primary = self.analyze(inner, input.clone());
                if let Some(catch) = catch {
                    primary.join(self.analyze(catch, input))
                } else {
                    primary
                }
            }
            Filter::Neg(inner) => match &inner.0 {
                Filter::Num(value) => StreamType::one(JType::number_lit(format!("-{value}"))),
                _ => StreamType::one(JType::number()),
            },
            Filter::Var(name) => match self.env.get(name).cloned() {
                Some(ty) => StreamType::one(ty),
                None => self.unsupported(
                    format!("variables are not supported yet: ${name}"),
                    span,
                    JType::Unknown,
                ),
            },
            Filter::Fold(fold_type, fold) => self.analyze_fold(*fold_type, fold, input),
            Filter::Recurse => {
                self.unsupported("recursive descent is not supported yet", span, input)
            }
        }
    }

    fn analyze_object(&mut self, items: &[KeyVal<SpannedFilter>], input: JType) -> StreamType {
        let mut properties = BTreeMap::new();
        let mut additional: Option<JType> = None;

        for item in items {
            match item {
                KeyVal::Filter(key_filter, value_filter) => {
                    let value = self.analyze(value_filter, input.clone()).item;
                    if let Some(key) = literal_string_filter(key_filter) {
                        properties.insert(
                            key,
                            Property {
                                ty: value,
                                required: true,
                            },
                        );
                    } else {
                        additional = Some(match additional {
                            Some(existing) => JType::union([existing, value]),
                            None => value,
                        });
                    }
                }
                KeyVal::Str(key, value_filter) => {
                    if let Some(key) = literal_string(key) {
                        let value = match value_filter {
                            Some(value_filter) => self.analyze(value_filter, input.clone()).item,
                            None => self.access_field(input.clone(), &key, false, 0..0).item,
                        };
                        properties.insert(
                            key,
                            Property {
                                ty: value,
                                required: true,
                            },
                        );
                    } else {
                        additional = Some(JType::Unknown);
                    }
                }
            }
        }

        StreamType::one(JType::object(properties, additional))
    }

    fn analyze_path(
        &mut self,
        base: &SpannedFilter,
        path: &[(Part<SpannedFilter>, Opt)],
        input: JType,
    ) -> StreamType {
        let mut stream = self.analyze(base, input);
        for (part, opt) in path {
            let part_stream = self.apply_path_part(stream.item, part, *opt);
            stream = StreamType::new(
                part_stream.item,
                stream.card.clone().compose(part_stream.card),
            );
        }
        stream
    }

    fn apply_path_part(
        &mut self,
        input: JType,
        part: &Part<SpannedFilter>,
        opt: Opt,
    ) -> StreamType {
        match part {
            Part::Index(index) => {
                if let Some(key) = literal_string_filter(index) {
                    self.access_field(input, &key, matches!(opt, Opt::Optional), index.1.clone())
                } else if let Some(index) = literal_i64_filter(index) {
                    self.access_index(
                        input,
                        index,
                        matches!(opt, Opt::Optional),
                        index_span(index),
                    )
                } else {
                    self.access_dynamic_index(input, matches!(opt, Opt::Optional), index.1.clone())
                }
            }
            Part::Range(None, None) => self.iterate(input, matches!(opt, Opt::Optional), 0..0),
            Part::Range(_, _) => {
                let item = match input {
                    JType::Array(array) => {
                        self.missing_path_null = false;
                        JType::array((*array.items).clone())
                    }
                    JType::String(_) => {
                        self.missing_path_null = false;
                        JType::string()
                    }
                    JType::Null if self.missing_path_null => {
                        self.missing_path_null = false;
                        JType::Unknown
                    }
                    JType::Unknown => {
                        self.missing_path_null = false;
                        JType::Unknown
                    }
                    JType::Union(items) => JType::union(
                        items
                            .into_iter()
                            .map(|item| self.apply_path_part(item, part, opt).item),
                    ),
                    other => {
                        self.warn_or_error(
                            format!(
                                "slice may be applied to non-array/non-string input: {}",
                                other.to_compact_string()
                            ),
                            None,
                        );
                        JType::Unknown
                    }
                };
                StreamType::one(item)
            }
        }
    }

    fn analyze_binary(
        &mut self,
        left: &SpannedFilter,
        op: &BinaryOp,
        right: &SpannedFilter,
        input: JType,
    ) -> StreamType {
        match op {
            BinaryOp::Pipe(binding) => {
                let left_stream = self.analyze(left, input.clone());
                if matches!(left_stream.card, Cardinality::Zero) {
                    return StreamType::zero();
                }
                let mut per_item: Option<StreamType> = None;
                for item in flatten_union(left_stream.item.clone()) {
                    let branch = if let Some(name) = binding {
                        self.with_var(name, item, |analyzer| {
                            analyzer.analyze(right, input.clone())
                        })
                    } else {
                        self.analyze(right, item)
                    };
                    per_item = Some(match per_item {
                        Some(existing) => existing.join_alternative(branch),
                        None => branch,
                    });
                }
                let per_item = per_item.unwrap_or_else(StreamType::zero);
                StreamType::new(per_item.item, left_stream.card.compose(per_item.card))
            }
            BinaryOp::Comma => self
                .analyze(left, input.clone())
                .join(self.analyze(right, input)),
            BinaryOp::Ord(_) | BinaryOp::Or | BinaryOp::And => StreamType::one(JType::bool()),
            BinaryOp::Math(op) => {
                let left = self.analyze(left, input.clone());
                let right = self.analyze(right, input);
                StreamType::new(
                    math_type(*op, left.item, right.item),
                    left.card.compose(right.card),
                )
            }
            BinaryOp::Alt => {
                let left = self.analyze(left, input.clone());
                let right = self.analyze(right, input);
                StreamType::new(
                    alt_type(left.item, right.item),
                    left.card.alternative(right.card),
                )
            }
            BinaryOp::Assign(op) => self.analyze_assignment(left, op, right, input),
        }
    }

    fn with_var<T>(&mut self, name: &str, ty: JType, f: impl FnOnce(&mut Self) -> T) -> T {
        let previous = self.env.insert(name.to_string(), ty);
        let out = f(self);
        match previous {
            Some(previous) => {
                self.env.insert(name.to_string(), previous);
            }
            None => {
                self.env.remove(name);
            }
        }
        out
    }

    fn with_fresh_missing_path_scope<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let previous = self.missing_path_null;
        self.missing_path_null = false;
        let out = f(self);
        self.missing_path_null = previous;
        out
    }

    fn analyze_fold(
        &mut self,
        fold_type: FoldType,
        fold: &jaq_syn::filter::Fold<Box<SpannedFilter>>,
        input: JType,
    ) -> StreamType {
        let xs = self.analyze(&fold.xs, input.clone());
        let init = self.analyze(&fold.init, input);

        if matches!(xs.card, Cardinality::Zero) {
            return init;
        }

        let mut acc = init.item.clone();
        let mut update_stream = StreamType::one(acc.clone());
        for _ in 0..8 {
            let mut per_item: Option<StreamType> = None;
            for item in flatten_union(xs.item.clone()) {
                let branch = self.with_var(&fold.x, item, |analyzer| {
                    analyzer.analyze(&fold.f, acc.clone())
                });
                per_item = Some(match per_item {
                    Some(existing) => existing.join_alternative(branch),
                    None => branch,
                });
            }
            update_stream = per_item.unwrap_or_else(StreamType::zero);
            let next = JType::union([acc.clone(), update_stream.item.clone()]);
            if next == acc {
                break;
            }
            acc = next;
        }

        let card = if matches!(init.card, Cardinality::One)
            && matches!(update_stream.card, Cardinality::One)
        {
            Cardinality::One
        } else {
            Cardinality::ZeroOrMore
        };

        match fold_type {
            FoldType::Reduce => StreamType::new(acc, card),
            FoldType::Foreach | FoldType::For => StreamType::new(acc, Cardinality::ZeroOrMore),
        }
    }

    fn analyze_assignment(
        &mut self,
        left: &SpannedFilter,
        op: &AssignOp,
        right: &SpannedFilter,
        input: JType,
    ) -> StreamType {
        match op {
            AssignOp::Assign => {
                let rhs = self.analyze(right, input.clone());
                let item = self.write_filter_path(input, left, rhs.item, left.1.clone());
                StreamType::new(item, rhs.card)
            }
            AssignOp::Update => {
                let old = self.analyze(left, input.clone());
                let rhs = self.analyze(right, old.item.clone());
                let item = self.write_filter_path(input, left, rhs.item, left.1.clone());
                StreamType::new(item, old.card.compose(rhs.card))
            }
            AssignOp::UpdateWith(op) => {
                let old = self.analyze(left, input.clone());
                let rhs = self.analyze(right, input.clone());
                let value = math_type(*op, old.item, rhs.item);
                let item = self.write_filter_path(input, left, value, left.1.clone());
                StreamType::new(item, old.card.compose(rhs.card))
            }
        }
    }

    fn analyze_if(
        &mut self,
        branches: &[(SpannedFilter, SpannedFilter)],
        else_branch: Option<&SpannedFilter>,
        input: JType,
    ) -> StreamType {
        let mut output: Option<StreamType> = None;
        let mut remaining = input;

        for (condition, then_branch) in branches {
            if remaining.is_never() {
                break;
            }

            let refinement = self.analyze_predicate(condition, remaining.clone());
            if !refinement.when_true.is_never() {
                let branch = self.analyze(then_branch, refinement.when_true);
                output = Some(match output {
                    Some(output) => output.join_alternative(branch),
                    None => branch,
                });
            }

            remaining = refinement.when_false;
        }

        if let Some(else_branch) = else_branch
            && !remaining.is_never()
        {
            let branch = self.analyze(else_branch, remaining);
            match output {
                Some(output) => output.join_alternative(branch),
                None => branch,
            }
        } else {
            output.unwrap_or_else(StreamType::zero)
        }
    }

    fn analyze_call(
        &mut self,
        name: &str,
        args: &[SpannedFilter],
        span: Span,
        input: JType,
    ) -> StreamType {
        match (name, args) {
            ("null", []) => StreamType::one(JType::Null),
            ("true", []) => StreamType::one(JType::bool_lit(true)),
            ("false", []) => StreamType::one(JType::bool_lit(false)),
            ("empty", []) => StreamType::zero(),
            ("type", []) => StreamType::one(JType::union(
                input.type_names().into_iter().map(JType::string_lit),
            )),
            ("length", []) => StreamType::one(JType::number()),
            ("tostring", []) => StreamType::one(tostring_type(input)),
            ("tonumber", []) => StreamType::one(tonumber_type(input)),
            ("floor", []) => StreamType::one(JType::number()),
            ("now", []) => StreamType::one(JType::number()),
            ("keys", []) => StreamType::one(self.keys_type(input)),
            ("not", []) => StreamType::one(not_type(input)),
            ("has", [key]) => StreamType::one(self.has_type(input, key)),
            ("select", [predicate]) => {
                let refinement = self.analyze_predicate(predicate, input);
                if refinement.when_true.is_never() {
                    StreamType::zero()
                } else if refinement.when_false.is_never() {
                    StreamType::one(refinement.when_true)
                } else {
                    StreamType::zero_or_one(refinement.when_true)
                }
            }
            ("map", [mapper]) => self.map_call(mapper, input),
            ("add", []) => StreamType::one(self.add_type(input)),
            ("join", [separator]) => {
                let _ = self.analyze(separator, input.clone());
                StreamType::one(JType::string())
            }
            ("transpose", []) => StreamType::one(self.transpose_type(input)),
            ("ascii_upcase", []) => StreamType::one(ascii_upcase_type(input)),
            ("values", []) => self.filter_values(input),
            ("nulls", []) => self.filter_kind(input, "null"),
            ("booleans", []) => self.filter_kind(input, "boolean"),
            ("numbers", []) => self.filter_kind(input, "number"),
            ("strings", []) => self.filter_kind(input, "string"),
            ("arrays", []) => self.filter_kind(input, "array"),
            ("objects", []) => self.filter_kind(input, "object"),
            _ => self.unsupported(
                format!("unsupported builtin or call `{name}`"),
                span,
                JType::Unknown,
            ),
        }
    }

    fn analyze_predicate(
        &mut self,
        predicate: &SpannedFilter,
        input: JType,
    ) -> PredicateRefinement {
        match &predicate.0 {
            Filter::Binary(left, BinaryOp::Ord(op @ (OrdOp::Eq | OrdOp::Ne)), right) => {
                if let Some(kind) = type_comparison_kind(left, right) {
                    return self.refine_type_predicate(input, &kind, *op);
                }
                if let Some(kind) = type_comparison_kind(right, left) {
                    return self.refine_type_predicate(input, &kind, *op);
                }
                if let (Some(field), Some(literal)) =
                    (top_level_field_access(left), literal_type_filter(right))
                {
                    return self.refine_field_literal_predicate(input, &field, literal, *op);
                }
                if let (Some(field), Some(literal)) =
                    (top_level_field_access(right), literal_type_filter(left))
                {
                    return self.refine_field_literal_predicate(input, &field, literal, *op);
                }
            }
            Filter::Binary(left, BinaryOp::And, right) => {
                let left = self.analyze_predicate(left, input);
                let right = self.analyze_predicate(right, left.when_true.clone());
                return PredicateRefinement {
                    when_true: right.when_true,
                    when_false: JType::union([left.when_false, right.when_false]),
                };
            }
            Filter::Binary(left, BinaryOp::Or, right) => {
                let left = self.analyze_predicate(left, input);
                let right = self.analyze_predicate(right, left.when_false.clone());
                return PredicateRefinement {
                    when_true: JType::union([left.when_true, right.when_true]),
                    when_false: right.when_false,
                };
            }
            Filter::Call(name, args) if name == "has" && args.len() == 1 => {
                if let Some(key) = literal_string_filter(&args[0]) {
                    return refine_has_predicate(input, &key);
                }
            }
            _ => {}
        }

        let output = self.analyze(predicate, input.clone());
        match output.item.is_truthy_literal() {
            Some(true) => PredicateRefinement {
                when_true: input,
                when_false: JType::Never,
            },
            Some(false) => PredicateRefinement {
                when_true: JType::Never,
                when_false: input,
            },
            None => PredicateRefinement {
                when_true: input.clone(),
                when_false: input,
            },
        }
    }

    fn refine_type_predicate(
        &mut self,
        input: JType,
        kind: &str,
        op: OrdOp,
    ) -> PredicateRefinement {
        let matching = narrow_by_type_name(input.clone(), kind);
        let non_matching = exclude_by_type_name(input, kind);
        match op {
            OrdOp::Eq => PredicateRefinement {
                when_true: matching,
                when_false: non_matching,
            },
            OrdOp::Ne => PredicateRefinement {
                when_true: non_matching,
                when_false: matching,
            },
            _ => unreachable!(),
        }
    }

    fn refine_field_literal_predicate(
        &mut self,
        input: JType,
        field: &str,
        literal: JType,
        op: OrdOp,
    ) -> PredicateRefinement {
        if matches!(literal, JType::Null) {
            let non_null = refine_field_non_null(input, field);
            return match op {
                OrdOp::Ne => non_null,
                OrdOp::Eq => PredicateRefinement {
                    when_true: non_null.when_false,
                    when_false: non_null.when_true,
                },
                _ => unreachable!(),
            };
        }

        let eq = refine_field_equals(input, field, literal);
        match op {
            OrdOp::Eq => eq,
            OrdOp::Ne => PredicateRefinement {
                when_true: eq.when_false,
                when_false: eq.when_true,
            },
            _ => unreachable!(),
        }
    }

    fn has_type(&mut self, input: JType, key: &SpannedFilter) -> JType {
        let Some(key) = literal_string_filter(key) else {
            return JType::bool();
        };

        let refinement = refine_has_predicate(input, &key);
        match (
            refinement.when_true.is_never(),
            refinement.when_false.is_never(),
        ) {
            (true, false) => JType::bool_lit(false),
            (false, true) => JType::bool_lit(true),
            _ => JType::bool(),
        }
    }

    fn keys_type(&mut self, input: JType) -> JType {
        match input {
            JType::Object(object) => {
                self.missing_path_null = false;
                let mut keys = object
                    .properties
                    .keys()
                    .cloned()
                    .map(JType::string_lit)
                    .collect::<Vec<_>>();
                if object.additional.is_some() {
                    keys.push(JType::string());
                }
                JType::array(JType::union(keys))
            }
            JType::Array(_) => {
                self.missing_path_null = false;
                JType::array(JType::number())
            }
            JType::Union(items) => JType::union(items.into_iter().map(|item| self.keys_type(item))),
            JType::Null if self.missing_path_null => {
                self.missing_path_null = false;
                JType::Unknown
            }
            JType::Unknown => {
                self.missing_path_null = false;
                JType::array(JType::union([JType::string(), JType::number()]))
            }
            other => {
                self.warn_or_error(
                    format!(
                        "keys may be applied to non-array/non-object input: {}",
                        other.to_compact_string()
                    ),
                    None,
                );
                JType::Unknown
            }
        }
    }

    fn map_call(&mut self, mapper: &SpannedFilter, input: JType) -> StreamType {
        match input {
            JType::Array(array) => {
                let mapped = self.with_fresh_missing_path_scope(|analyzer| {
                    analyzer.analyze(mapper, *array.items)
                });
                self.missing_path_null = false;
                StreamType::one(JType::array(mapped.item))
            }
            JType::Object(object) => {
                let mut values = object
                    .properties
                    .values()
                    .map(|prop| prop.ty.clone())
                    .collect::<Vec<_>>();
                if let Some(additional) = object.additional {
                    values.push(*additional);
                }
                let mapped = self.with_fresh_missing_path_scope(|analyzer| {
                    analyzer.analyze(mapper, JType::union(values))
                });
                self.missing_path_null = false;
                StreamType::one(JType::array(mapped.item))
            }
            JType::Union(items) => {
                let mapped = JType::union(
                    items
                        .into_iter()
                        .map(|item| self.map_call(mapper, item).item),
                );
                self.missing_path_null = false;
                StreamType::one(mapped)
            }
            JType::Unknown => {
                self.missing_path_null = false;
                StreamType::one(JType::array(JType::Unknown))
            }
            JType::Null if self.missing_path_null => {
                self.missing_path_null = false;
                StreamType::one(JType::Unknown)
            }
            other => {
                self.missing_path_null = false;
                self.warn_or_error(
                    format!(
                        "map may be applied to non-array/non-object input: {}",
                        other.to_compact_string()
                    ),
                    None,
                );
                StreamType::one(JType::Unknown)
            }
        }
    }

    fn add_type(&mut self, input: JType) -> JType {
        match input {
            JType::Array(array) => {
                self.missing_path_null = false;
                let item = *array.items;
                if item.is_never() {
                    JType::Null
                } else {
                    JType::union([math_type(MathOp::Add, item.clone(), item), JType::Null])
                }
            }
            JType::Union(items) => JType::union(items.into_iter().map(|item| self.add_type(item))),
            JType::Null if self.missing_path_null => {
                self.missing_path_null = false;
                JType::Unknown
            }
            JType::Unknown => {
                self.missing_path_null = false;
                JType::Unknown
            }
            other => {
                self.warn_or_error(
                    format!(
                        "add may be applied to non-array input: {}",
                        other.to_compact_string()
                    ),
                    None,
                );
                JType::Unknown
            }
        }
    }

    fn transpose_type(&mut self, input: JType) -> JType {
        match input {
            JType::Array(array) => {
                self.missing_path_null = false;
                match *array.items {
                    JType::Array(inner) => JType::array(JType::array(*inner.items)),
                    JType::Unknown => JType::array(JType::array(JType::Unknown)),
                    other => {
                        self.warn_or_error(
                            format!(
                                "transpose may be applied to non-array items: {}",
                                other.to_compact_string()
                            ),
                            None,
                        );
                        JType::array(JType::array(JType::Unknown))
                    }
                }
            }
            JType::Union(items) => {
                JType::union(items.into_iter().map(|item| self.transpose_type(item)))
            }
            JType::Null if self.missing_path_null => {
                self.missing_path_null = false;
                JType::Unknown
            }
            JType::Unknown => {
                self.missing_path_null = false;
                JType::array(JType::array(JType::Unknown))
            }
            other => {
                self.warn_or_error(
                    format!(
                        "transpose may be applied to non-array input: {}",
                        other.to_compact_string()
                    ),
                    None,
                );
                JType::Unknown
            }
        }
    }

    fn access_field(&mut self, input: JType, key: &str, optional: bool, span: Span) -> StreamType {
        match input {
            JType::Object(object) => {
                if let Some(prop) = object.properties.get(key) {
                    self.missing_path_null = false;
                    if prop.required {
                        StreamType::one(prop.ty.clone())
                    } else {
                        StreamType::one(JType::union([prop.ty.clone(), JType::Null]))
                    }
                } else if let Some(additional) = object.additional {
                    self.missing_path_null = false;
                    StreamType::one(JType::union([*additional, JType::Null]))
                } else {
                    if optional {
                        self.missing_path_null = false;
                    } else {
                        self.warn_or_error(
                            format!(
                                "property \"{key}\" is not present on {}",
                                JType::Object(object).to_compact_string()
                            ),
                            Some(span),
                        );
                        self.missing_path_null = true;
                    }
                    StreamType::one(JType::Null)
                }
            }
            JType::Null => StreamType::one(JType::Null),
            JType::Unknown => {
                self.missing_path_null = false;
                StreamType::one(JType::Unknown)
            }
            JType::Union(items) => {
                let mut out = StreamType::zero();
                for item in items {
                    out = out.join(self.access_field(item, key, optional, span.clone()));
                }
                out
            }
            other if optional => {
                self.missing_path_null = false;
                self.warn_or_error(
                    format!(
                        "optional field `{key}` skipped non-object input: {}",
                        other.to_compact_string()
                    ),
                    Some(span),
                );
                StreamType::zero()
            }
            other => {
                self.missing_path_null = false;
                self.warn_or_error(
                    format!(
                        "field `{key}` may be applied to non-object input: {}",
                        other.to_compact_string()
                    ),
                    Some(span),
                );
                StreamType::one(JType::Unknown)
            }
        }
    }

    fn access_index(
        &mut self,
        input: JType,
        _index: i64,
        optional: bool,
        span: Span,
    ) -> StreamType {
        match input {
            JType::Array(array) => {
                self.missing_path_null = false;
                StreamType::one(JType::union([*array.items, JType::Null]))
            }
            JType::String(_) => {
                self.missing_path_null = false;
                StreamType::one(JType::union([JType::string(), JType::Null]))
            }
            JType::Null if self.missing_path_null => {
                self.missing_path_null = false;
                StreamType::one(JType::Unknown)
            }
            JType::Unknown => {
                self.missing_path_null = false;
                StreamType::one(JType::Unknown)
            }
            JType::Union(items) => {
                let mut out = StreamType::zero();
                for item in items {
                    out = out.join(self.access_index(item, _index, optional, span.clone()));
                }
                out
            }
            other if optional => {
                self.warn_or_error(
                    format!(
                        "optional index skipped non-array input: {}",
                        other.to_compact_string()
                    ),
                    Some(span),
                );
                StreamType::zero()
            }
            other => {
                self.warn_or_error(
                    format!(
                        "array index may be applied to non-array input: {}",
                        other.to_compact_string()
                    ),
                    Some(span),
                );
                StreamType::one(JType::Unknown)
            }
        }
    }

    fn access_dynamic_index(&mut self, input: JType, optional: bool, span: Span) -> StreamType {
        match input {
            JType::Array(array) => {
                self.missing_path_null = false;
                StreamType::one(JType::union([*array.items, JType::Null]))
            }
            JType::Object(object) => {
                self.missing_path_null = false;
                let mut values = object
                    .properties
                    .values()
                    .map(|prop| prop.ty.clone())
                    .collect::<Vec<_>>();
                if let Some(additional) = object.additional {
                    values.push(*additional);
                }
                values.push(JType::Null);
                StreamType::one(JType::union(values))
            }
            JType::Null if self.missing_path_null => {
                self.missing_path_null = false;
                StreamType::one(JType::Unknown)
            }
            JType::Unknown => {
                self.missing_path_null = false;
                StreamType::one(JType::Unknown)
            }
            JType::Union(items) => {
                let mut out = StreamType::zero();
                for item in items {
                    out = out.join(self.access_dynamic_index(item, optional, span.clone()));
                }
                out
            }
            other if optional => {
                self.warn_or_error(
                    format!(
                        "optional dynamic index skipped input: {}",
                        other.to_compact_string()
                    ),
                    Some(span),
                );
                StreamType::zero()
            }
            other => {
                self.warn_or_error(
                    format!(
                        "dynamic index may be applied to non-container input: {}",
                        other.to_compact_string()
                    ),
                    Some(span),
                );
                StreamType::one(JType::Unknown)
            }
        }
    }

    fn iterate(&mut self, input: JType, optional: bool, span: Span) -> StreamType {
        match input {
            JType::Array(array) => {
                self.missing_path_null = false;
                StreamType::zero_or_more(*array.items)
            }
            JType::Object(object) => {
                self.missing_path_null = false;
                let mut values = object
                    .properties
                    .values()
                    .map(|prop| prop.ty.clone())
                    .collect::<Vec<_>>();
                if let Some(additional) = object.additional {
                    values.push(*additional);
                }
                StreamType::zero_or_more(JType::union(values))
            }
            JType::Null if self.missing_path_null => {
                self.missing_path_null = false;
                StreamType::zero_or_more(JType::Unknown)
            }
            JType::Unknown => {
                self.missing_path_null = false;
                StreamType::zero_or_more(JType::Unknown)
            }
            JType::Union(items) => {
                let mut out = StreamType::zero();
                for item in items {
                    out = out.join(self.iterate(item, optional, span.clone()));
                }
                out
            }
            other if optional => {
                self.warn_or_error(
                    format!(
                        "optional iteration skipped non-iterable input: {}",
                        other.to_compact_string()
                    ),
                    Some(span),
                );
                StreamType::zero()
            }
            other => {
                self.warn_or_error(
                    format!(
                        "iteration may be applied to non-iterable input: {}",
                        other.to_compact_string()
                    ),
                    Some(span),
                );
                StreamType::zero_or_more(JType::Unknown)
            }
        }
    }

    fn filter_values(&mut self, input: JType) -> StreamType {
        let non_null = input.without_null();
        if non_null.is_never() {
            StreamType::zero()
        } else {
            StreamType::zero_or_one(non_null)
        }
    }

    fn filter_kind(&mut self, input: JType, kind: &str) -> StreamType {
        let matching = narrow_by_type_name(input.clone(), kind);
        if matching.is_never() {
            StreamType::zero()
        } else if exclude_by_type_name(input, kind).is_never() {
            StreamType::one(matching)
        } else {
            StreamType::zero_or_one(matching)
        }
    }

    fn string_type(&mut self, value: &jaq_syn::Str<SpannedFilter>, input: JType) -> JType {
        if let Some(literal) = literal_string(value) {
            return JType::string_lit(literal);
        }
        for part in &value.parts {
            if let string::Part::Fun(filter) = part {
                let _ = self.analyze(filter, input.clone());
            }
        }
        JType::string()
    }

    fn write_filter_path(
        &mut self,
        input: JType,
        path_filter: &SpannedFilter,
        value: JType,
        span: Span,
    ) -> JType {
        match &path_filter.0 {
            Filter::Id => value,
            Filter::Path(base, path) if matches!(base.0, Filter::Id) => {
                self.write_path_parts(input, path, value, span)
            }
            _ => {
                self.warn_or_error(
                    "assignment left-hand side is not a supported identity-root path",
                    Some(span),
                );
                JType::Unknown
            }
        }
    }

    fn write_path_parts(
        &mut self,
        input: JType,
        path: &[(Part<SpannedFilter>, Opt)],
        value: JType,
        span: Span,
    ) -> JType {
        let Some((head, rest)) = path.split_first() else {
            return value;
        };

        match &head.0 {
            Part::Index(index) => {
                if let Some(key) = literal_string_filter(index) {
                    self.write_field(input, &key, rest, value, index.1.clone())
                } else if literal_i64_filter(index).is_some() {
                    self.write_array_index(input, rest, value, index.1.clone())
                } else {
                    let key_type = self.analyze(index, input.clone()).item;
                    self.write_dynamic_index(input, key_type, rest, value, index.1.clone())
                }
            }
            Part::Range(None, None) => self.write_array_index(input, rest, value, span),
            Part::Range(_, _) => {
                self.warn_or_error(
                    "slice assignment is not supported precisely yet",
                    Some(span),
                );
                JType::Unknown
            }
        }
    }

    fn write_field(
        &mut self,
        input: JType,
        key: &str,
        rest: &[(Part<SpannedFilter>, Opt)],
        value: JType,
        span: Span,
    ) -> JType {
        match input {
            JType::Object(mut object) => {
                let old = object
                    .properties
                    .get(key)
                    .map(|prop| prop.ty.clone())
                    .or_else(|| {
                        object
                            .additional
                            .as_ref()
                            .map(|additional| (**additional).clone())
                    })
                    .unwrap_or(JType::Unknown);
                let written = self.write_path_parts(old, rest, value, span);
                object.properties.insert(
                    key.to_string(),
                    Property {
                        ty: written,
                        required: true,
                    },
                );
                JType::Object(object)
            }
            JType::Null => {
                let written = self.write_path_parts(JType::Unknown, rest, value, span);
                let mut properties = BTreeMap::new();
                properties.insert(
                    key.to_string(),
                    Property {
                        ty: written,
                        required: true,
                    },
                );
                JType::closed_object(properties)
            }
            JType::Unknown => {
                let written = self.write_path_parts(JType::Unknown, rest, value, span);
                let mut properties = BTreeMap::new();
                properties.insert(
                    key.to_string(),
                    Property {
                        ty: written,
                        required: true,
                    },
                );
                JType::open_object(properties)
            }
            JType::Union(items) => JType::union(
                items
                    .into_iter()
                    .map(|item| self.write_field(item, key, rest, value.clone(), span.clone())),
            ),
            other => {
                self.warn_or_error(
                    format!(
                        "field assignment may be applied to non-object input: {}",
                        other.to_compact_string()
                    ),
                    Some(span),
                );
                JType::Unknown
            }
        }
    }

    fn write_array_index(
        &mut self,
        input: JType,
        rest: &[(Part<SpannedFilter>, Opt)],
        value: JType,
        span: Span,
    ) -> JType {
        match input {
            JType::Array(array) => {
                let written = self.write_path_parts(*array.items, rest, value, span);
                JType::array(written)
            }
            JType::Null => {
                let written = self.write_path_parts(JType::Unknown, rest, value, span);
                JType::array(written)
            }
            JType::Unknown => {
                let written = self.write_path_parts(JType::Unknown, rest, value, span);
                JType::array(written)
            }
            JType::Union(items) => JType::union(
                items
                    .into_iter()
                    .map(|item| self.write_array_index(item, rest, value.clone(), span.clone())),
            ),
            other => {
                self.warn_or_error(
                    format!(
                        "array assignment may be applied to non-array input: {}",
                        other.to_compact_string()
                    ),
                    Some(span),
                );
                JType::Unknown
            }
        }
    }

    fn write_dynamic_index(
        &mut self,
        input: JType,
        key_type: JType,
        rest: &[(Part<SpannedFilter>, Opt)],
        value: JType,
        span: Span,
    ) -> JType {
        if !rest.is_empty() {
            self.warn_or_error(
                "nested dynamic assignment is not supported precisely yet",
                Some(span.clone()),
            );
            return JType::Unknown;
        }

        if let Some(keys) = finite_string_literals(&key_type) {
            let mut out = input;
            for key in keys {
                out = self.write_field(out, &key, rest, value.clone(), span.clone());
            }
            return out;
        }

        if is_string_like(&key_type) {
            return self.write_dynamic_object_key(input, value, span);
        }

        if is_number_like(&key_type) {
            return self.write_array_index(input, rest, value, span);
        }

        match input {
            JType::Unknown => JType::Unknown,
            JType::Union(items) => JType::union(items.into_iter().map(|item| {
                self.write_dynamic_index(item, key_type.clone(), rest, value.clone(), span.clone())
            })),
            other => {
                self.warn_or_error(
                    format!(
                        "dynamic assignment key may be non-string/non-number: {}",
                        key_type.to_compact_string()
                    ),
                    Some(span.clone()),
                );
                JType::union([
                    self.write_dynamic_object_key(other, value, span),
                    JType::Unknown,
                ])
            }
        }
    }

    fn write_dynamic_object_key(&mut self, input: JType, value: JType, span: Span) -> JType {
        match input {
            JType::Object(mut object) => {
                for prop in object.properties.values_mut() {
                    prop.ty = JType::union([prop.ty.clone(), value.clone()]);
                }
                let additional = object
                    .additional
                    .map(|existing| JType::union([*existing, value.clone()]))
                    .unwrap_or(value);
                object.additional = Some(Box::new(additional));
                JType::Object(object)
            }
            JType::Null => JType::object(BTreeMap::new(), Some(value)),
            JType::Unknown => JType::object(BTreeMap::new(), Some(value)),
            JType::Union(items) => JType::union(
                items
                    .into_iter()
                    .map(|item| self.write_dynamic_object_key(item, value.clone(), span.clone())),
            ),
            other => {
                self.warn_or_error(
                    format!(
                        "dynamic object assignment may be applied to non-object input: {}",
                        other.to_compact_string()
                    ),
                    Some(span),
                );
                JType::Unknown
            }
        }
    }

    fn unsupported(
        &mut self,
        feature: impl Into<String>,
        span: Span,
        fallback: JType,
    ) -> StreamType {
        let feature = feature.into();
        let source_span = Some(SourceSpan::from_range(span));
        self.unsupported_features.push(UnsupportedFeature {
            feature: feature.clone(),
            span: source_span.clone(),
        });
        self.warn_or_error(feature, source_span.map(|s| s.start..s.end));
        StreamType::one(fallback)
    }

    fn warn_or_error(&mut self, message: impl Into<String>, span: Option<Span>) {
        let span = span.map(SourceSpan::from_range);
        let diagnostic = match self.options.mode {
            AnalysisMode::Permissive => Diagnostic::warning(message, span),
            AnalysisMode::Strict => Diagnostic::error(message, span),
        }
        .with_source_name(self.options.source_name.clone());
        self.diagnostics.push(diagnostic);
    }
}

fn literal_string(value: &jaq_syn::Str<SpannedFilter>) -> Option<String> {
    if value.fmt.is_some() {
        return None;
    }
    let mut out = String::new();
    for part in &value.parts {
        match part {
            string::Part::Str(value) => out.push_str(value),
            string::Part::Fun(_) => return None,
        }
    }
    Some(out)
}

fn math_type(op: MathOp, left: JType, right: JType) -> JType {
    match op {
        MathOp::Add => plus_type(left, right),
        MathOp::Sub | MathOp::Mul | MathOp::Div | MathOp::Rem => numeric_math_type(left, right),
    }
}

fn numeric_math_type(left: JType, right: JType) -> JType {
    match (left, right) {
        (JType::Union(left), right) => JType::union(
            left.into_iter()
                .map(|left| numeric_math_type(left, right.clone())),
        ),
        (left, JType::Union(right)) => JType::union(
            right
                .into_iter()
                .map(|right| numeric_math_type(left.clone(), right)),
        ),
        (JType::Unknown, _) | (_, JType::Unknown) => JType::Unknown,
        (JType::Number(_), JType::Number(_)) => JType::number(),
        (JType::Null, JType::Number(_)) | (JType::Number(_), JType::Null) => JType::number(),
        (JType::Null, JType::Null) => JType::number(),
        _ => JType::Unknown,
    }
}

fn plus_type(left: JType, right: JType) -> JType {
    match (left, right) {
        (JType::Union(left), right) => {
            JType::union(left.into_iter().map(|left| plus_type(left, right.clone())))
        }
        (left, JType::Union(right)) => JType::union(
            right
                .into_iter()
                .map(|right| plus_type(left.clone(), right)),
        ),
        (JType::Unknown, _) | (_, JType::Unknown) => JType::Unknown,
        (JType::Null, right) => right,
        (left, JType::Null) => left,
        (JType::Number(_), JType::Number(_)) => JType::number(),
        (JType::String(StringType::Literal(left)), JType::String(StringType::Literal(right))) => {
            JType::string_lit(format!("{left}{right}"))
        }
        (JType::String(_), JType::String(_)) => JType::string(),
        (JType::Array(left), JType::Array(right)) => {
            JType::array(JType::union([*left.items, *right.items]))
        }
        (JType::Object(left), JType::Object(right)) => JType::Object(merge_objects(left, right)),
        _ => JType::Unknown,
    }
}

fn merge_objects(
    mut left: crate::types::ObjectType,
    right: crate::types::ObjectType,
) -> crate::types::ObjectType {
    let right_additional = right
        .additional
        .as_ref()
        .map(|additional| (**additional).clone());
    if let Some(additional) = &right_additional {
        for prop in left.properties.values_mut() {
            prop.ty = JType::union([prop.ty.clone(), additional.clone()]);
        }
    }

    for (key, prop) in right.properties {
        left.properties.insert(key, prop);
    }

    left.additional = match (left.additional, right.additional) {
        (Some(left), Some(right)) => Some(Box::new(JType::union([*left, *right]))),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    };
    left
}

fn alt_type(left: JType, right: JType) -> JType {
    if matches!(left, JType::Unknown) {
        return JType::Unknown;
    }
    let left_without_falsey = without_null_false(left.clone());
    if left_without_falsey.is_never() {
        right
    } else if left_without_falsey == left {
        left
    } else {
        JType::union([left_without_falsey, right])
    }
}

fn without_null_false(input: JType) -> JType {
    match input {
        JType::Null => JType::Never,
        JType::Bool(BoolType::Literal(false)) => JType::Never,
        JType::Bool(BoolType::Literal(true)) => JType::bool_lit(true),
        JType::Bool(BoolType::Any) => JType::bool_lit(true),
        JType::Union(items) => JType::union(items.into_iter().map(without_null_false)),
        other => other,
    }
}

fn tostring_type(input: JType) -> JType {
    match input {
        JType::Union(items) => JType::union(items.into_iter().map(tostring_type)),
        JType::String(string) => JType::String(string),
        JType::Number(NumberType::Literal(value)) => JType::string_lit(value),
        JType::Number(NumberType::Any) => JType::string(),
        JType::Bool(BoolType::Literal(value)) => JType::string_lit(value.to_string()),
        JType::Bool(BoolType::Any) => {
            JType::union([JType::string_lit("true"), JType::string_lit("false")])
        }
        JType::Null => JType::string_lit("null"),
        JType::Never => JType::Never,
        JType::Unknown | JType::Array(_) | JType::Object(_) => JType::string(),
    }
}

fn tonumber_type(input: JType) -> JType {
    match input {
        JType::Union(items) => JType::union(items.into_iter().map(tonumber_type)),
        JType::Number(number) => JType::Number(number),
        JType::String(StringType::Literal(value)) if value.parse::<f64>().is_ok() => {
            JType::number_lit(value)
        }
        JType::String(_) | JType::Unknown => JType::number(),
        JType::Never => JType::Never,
        _ => JType::number(),
    }
}

fn not_type(input: JType) -> JType {
    match input.is_truthy_literal() {
        Some(value) => JType::bool_lit(!value),
        None => JType::bool(),
    }
}

fn ascii_upcase_type(input: JType) -> JType {
    match input {
        JType::Union(items) => JType::union(items.into_iter().map(ascii_upcase_type)),
        JType::String(StringType::Literal(value)) => JType::string_lit(value.to_ascii_uppercase()),
        JType::Never => JType::Never,
        _ => JType::string(),
    }
}

fn finite_string_literals(input: &JType) -> Option<Vec<String>> {
    match input {
        JType::String(StringType::Literal(value)) => Some(vec![value.clone()]),
        JType::Union(items) => {
            let mut out = Vec::new();
            for item in items {
                out.extend(finite_string_literals(item)?);
            }
            Some(out)
        }
        _ => None,
    }
}

fn is_string_like(input: &JType) -> bool {
    match input {
        JType::String(_) => true,
        JType::Union(items) => items.iter().all(is_string_like),
        _ => false,
    }
}

fn is_number_like(input: &JType) -> bool {
    match input {
        JType::Number(_) => true,
        JType::Union(items) => items.iter().all(is_number_like),
        _ => false,
    }
}

fn literal_string_filter(filter: &SpannedFilter) -> Option<String> {
    match &filter.0 {
        Filter::Str(value) => literal_string(value),
        _ => None,
    }
}

fn literal_i64_filter(filter: &SpannedFilter) -> Option<i64> {
    match &filter.0 {
        Filter::Num(value) => value.parse().ok(),
        _ => None,
    }
}

fn literal_type_filter(filter: &SpannedFilter) -> Option<JType> {
    match &filter.0 {
        Filter::Str(value) => literal_string(value).map(JType::string_lit),
        Filter::Num(value) => Some(JType::number_lit(value.clone())),
        Filter::Call(name, args) if name == "null" && args.is_empty() => Some(JType::Null),
        Filter::Call(name, args) if name == "true" && args.is_empty() => {
            Some(JType::bool_lit(true))
        }
        Filter::Call(name, args) if name == "false" && args.is_empty() => {
            Some(JType::bool_lit(false))
        }
        Filter::Neg(inner) => match &inner.0 {
            Filter::Num(value) => Some(JType::number_lit(format!("-{value}"))),
            _ => None,
        },
        _ => None,
    }
}

fn type_comparison_kind(type_filter: &SpannedFilter, literal: &SpannedFilter) -> Option<String> {
    match (&type_filter.0, literal_type_filter(literal)) {
        (Filter::Call(name, args), Some(JType::String(StringType::Literal(kind))))
            if name == "type" && args.is_empty() =>
        {
            Some(kind)
        }
        _ => None,
    }
}

fn top_level_field_access(filter: &SpannedFilter) -> Option<String> {
    let Filter::Path(base, path) = &filter.0 else {
        return None;
    };
    if !matches!(base.0, Filter::Id) || path.len() != 1 {
        return None;
    }
    let (Part::Index(index), _) = &path[0] else {
        return None;
    };
    literal_string_filter(index)
}

fn refine_has_predicate(input: JType, key: &str) -> PredicateRefinement {
    match input {
        JType::Object(mut object) => {
            if let Some(prop) = object.properties.get_mut(key) {
                if prop.required {
                    PredicateRefinement {
                        when_true: JType::Object(object),
                        when_false: JType::Never,
                    }
                } else {
                    prop.required = true;
                    let when_true = JType::Object(object.clone());
                    let mut false_object = object;
                    false_object.properties.remove(key);
                    PredicateRefinement {
                        when_true,
                        when_false: JType::Object(false_object),
                    }
                }
            } else if object.additional.is_some() {
                let mut true_object = object.clone();
                true_object.properties.insert(
                    key.to_string(),
                    Property {
                        ty: JType::Unknown,
                        required: true,
                    },
                );
                PredicateRefinement {
                    when_true: JType::Object(true_object),
                    when_false: JType::Object(object),
                }
            } else {
                PredicateRefinement {
                    when_true: JType::Never,
                    when_false: JType::Object(object),
                }
            }
        }
        JType::Union(items) => {
            let refinements = items
                .into_iter()
                .map(|item| refine_has_predicate(item, key))
                .collect::<Vec<_>>();
            PredicateRefinement {
                when_true: JType::union(
                    refinements
                        .iter()
                        .map(|refinement| refinement.when_true.clone()),
                ),
                when_false: JType::union(
                    refinements
                        .into_iter()
                        .map(|refinement| refinement.when_false),
                ),
            }
        }
        JType::Unknown => {
            let mut properties = BTreeMap::new();
            properties.insert(
                key.to_string(),
                Property {
                    ty: JType::Unknown,
                    required: true,
                },
            );
            PredicateRefinement {
                when_true: JType::open_object(properties),
                when_false: JType::Unknown,
            }
        }
        other => PredicateRefinement {
            when_true: JType::Never,
            when_false: other,
        },
    }
}

fn refine_field_equals(input: JType, field: &str, literal: JType) -> PredicateRefinement {
    match input {
        JType::Object(mut object) => {
            if let Some(prop) = object.properties.get(field) {
                let true_ty = intersect_type(prop.ty.clone(), literal.clone());
                let false_ty = exclude_literal_type(prop.ty.clone(), literal);

                let when_true = if true_ty.is_never() {
                    JType::Never
                } else {
                    let mut true_object = object.clone();
                    true_object.properties.insert(
                        field.to_string(),
                        Property {
                            ty: true_ty,
                            required: true,
                        },
                    );
                    JType::Object(true_object)
                };

                let when_false = if false_ty.is_never() && prop.required {
                    JType::Never
                } else {
                    if false_ty.is_never() {
                        object.properties.remove(field);
                    } else {
                        object.properties.insert(
                            field.to_string(),
                            Property {
                                ty: false_ty,
                                required: prop.required,
                            },
                        );
                    }
                    JType::Object(object)
                };

                PredicateRefinement {
                    when_true,
                    when_false,
                }
            } else if object.additional.is_some() {
                let when_true = if matches!(literal, JType::Null) {
                    JType::Object(object.clone())
                } else {
                    let mut true_object = object.clone();
                    true_object.properties.insert(
                        field.to_string(),
                        Property {
                            ty: literal,
                            required: true,
                        },
                    );
                    JType::Object(true_object)
                };
                PredicateRefinement {
                    when_true,
                    when_false: JType::Object(object),
                }
            } else if matches!(literal, JType::Null) {
                PredicateRefinement {
                    when_true: JType::Object(object),
                    when_false: JType::Never,
                }
            } else {
                PredicateRefinement {
                    when_true: JType::Never,
                    when_false: JType::Object(object),
                }
            }
        }
        JType::Null if matches!(literal, JType::Null) => PredicateRefinement {
            when_true: JType::Null,
            when_false: JType::Never,
        },
        JType::Union(items) => {
            let refinements = items
                .into_iter()
                .map(|item| refine_field_equals(item, field, literal.clone()))
                .collect::<Vec<_>>();
            PredicateRefinement {
                when_true: JType::union(
                    refinements
                        .iter()
                        .map(|refinement| refinement.when_true.clone()),
                ),
                when_false: JType::union(
                    refinements
                        .into_iter()
                        .map(|refinement| refinement.when_false),
                ),
            }
        }
        JType::Unknown => {
            let mut properties = BTreeMap::new();
            properties.insert(
                field.to_string(),
                Property {
                    ty: literal,
                    required: true,
                },
            );
            PredicateRefinement {
                when_true: JType::open_object(properties),
                when_false: JType::Unknown,
            }
        }
        other => PredicateRefinement {
            when_true: JType::Never,
            when_false: other,
        },
    }
}

fn refine_field_non_null(input: JType, field: &str) -> PredicateRefinement {
    match input {
        JType::Object(mut object) => {
            if let Some(prop) = object.properties.get(field) {
                let non_null = prop.ty.clone().without_null();
                let null_part = intersect_type(prop.ty.clone(), JType::Null);

                let when_true = if non_null.is_never() {
                    JType::Never
                } else {
                    let mut true_object = object.clone();
                    true_object.properties.insert(
                        field.to_string(),
                        Property {
                            ty: non_null,
                            required: true,
                        },
                    );
                    JType::Object(true_object)
                };

                let when_false = if prop.required && null_part.is_never() {
                    JType::Never
                } else {
                    if null_part.is_never() {
                        object.properties.remove(field);
                    } else {
                        object.properties.insert(
                            field.to_string(),
                            Property {
                                ty: JType::Null,
                                required: prop.required,
                            },
                        );
                    }
                    JType::Object(object)
                };

                PredicateRefinement {
                    when_true,
                    when_false,
                }
            } else if object.additional.is_some() {
                let mut true_object = object.clone();
                true_object.properties.insert(
                    field.to_string(),
                    Property {
                        ty: JType::Unknown,
                        required: true,
                    },
                );
                PredicateRefinement {
                    when_true: JType::Object(true_object),
                    when_false: JType::Object(object),
                }
            } else {
                PredicateRefinement {
                    when_true: JType::Never,
                    when_false: JType::Object(object),
                }
            }
        }
        JType::Union(items) => {
            let refinements = items
                .into_iter()
                .map(|item| refine_field_non_null(item, field))
                .collect::<Vec<_>>();
            PredicateRefinement {
                when_true: JType::union(
                    refinements
                        .iter()
                        .map(|refinement| refinement.when_true.clone()),
                ),
                when_false: JType::union(
                    refinements
                        .into_iter()
                        .map(|refinement| refinement.when_false),
                ),
            }
        }
        JType::Unknown => {
            let mut properties = BTreeMap::new();
            properties.insert(
                field.to_string(),
                Property {
                    ty: JType::Unknown,
                    required: true,
                },
            );
            PredicateRefinement {
                when_true: JType::open_object(properties),
                when_false: JType::Unknown,
            }
        }
        other => PredicateRefinement {
            when_true: JType::Never,
            when_false: other,
        },
    }
}

fn intersect_type(ty: JType, literal: JType) -> JType {
    match (ty, literal) {
        (JType::Unknown, literal) => literal,
        (JType::Union(items), literal) => JType::union(
            items
                .into_iter()
                .map(|item| intersect_type(item, literal.clone())),
        ),
        (JType::Null, JType::Null) => JType::Null,
        (JType::Bool(BoolType::Any), literal @ JType::Bool(_)) => literal,
        (JType::Bool(BoolType::Literal(a)), JType::Bool(BoolType::Literal(b))) if a == b => {
            JType::bool_lit(a)
        }
        (JType::Number(NumberType::Any), literal @ JType::Number(_)) => literal,
        (JType::Number(NumberType::Literal(a)), JType::Number(NumberType::Literal(b)))
            if a == b =>
        {
            JType::number_lit(a)
        }
        (JType::String(StringType::Any), literal @ JType::String(_)) => literal,
        (JType::String(StringType::Literal(a)), JType::String(StringType::Literal(b)))
            if a == b =>
        {
            JType::string_lit(a)
        }
        (left, right) if left == right => left,
        _ => JType::Never,
    }
}

fn exclude_literal_type(ty: JType, literal: JType) -> JType {
    match ty {
        JType::Union(items) => JType::union(
            items
                .into_iter()
                .map(|item| exclude_literal_type(item, literal.clone())),
        ),
        ty if intersect_type(ty.clone(), literal).is_never() => ty,
        JType::Bool(BoolType::Literal(_))
        | JType::Number(NumberType::Literal(_))
        | JType::String(StringType::Literal(_))
        | JType::Null => JType::Never,
        other => other,
    }
}

fn narrow_by_type_name(input: JType, kind: &str) -> JType {
    match input {
        JType::Unknown => kind_to_type(kind),
        JType::Union(items) => JType::union(
            items
                .into_iter()
                .map(|item| narrow_by_type_name(item, kind)),
        ),
        item if item.type_names().iter().any(|name| name == kind) => item,
        _ => JType::Never,
    }
}

fn exclude_by_type_name(input: JType, kind: &str) -> JType {
    match input {
        JType::Unknown => JType::Unknown,
        JType::Union(items) => JType::union(
            items
                .into_iter()
                .map(|item| exclude_by_type_name(item, kind)),
        ),
        item if item.type_names().iter().any(|name| name == kind) => JType::Never,
        item => item,
    }
}

fn kind_to_type(kind: &str) -> JType {
    match kind {
        "null" => JType::Null,
        "boolean" => JType::bool(),
        "number" => JType::number(),
        "string" => JType::string(),
        "array" => JType::array(JType::Unknown),
        "object" => JType::open_object(BTreeMap::new()),
        _ => JType::Never,
    }
}

fn flatten_union(input: JType) -> Vec<JType> {
    match input {
        JType::Union(items) => items,
        other => vec![other],
    }
}

fn index_span(_index: i64) -> Span {
    0..0
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn check(filter: &str, input: JType) -> AnalyzeReport {
        JqTypeChecker::new().analyze_filter(
            filter,
            InputShape::Type(input),
            AnalyzeOptions::default(),
        )
    }

    #[test]
    fn analyzes_field_projection() {
        let mut props = BTreeMap::new();
        props.insert("name".to_string(), JType::property(JType::string(), true));
        let report = check(".name", JType::closed_object(props));
        assert_eq!(report.output.to_compact_string(), "string");
    }

    #[test]
    fn analyzes_array_collection() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": { "name": { "type": "string" } },
                        "required": ["name"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["items"],
            "additionalProperties": false
        }));

        let report = check("[.items[].name]", input);
        assert_eq!(report.output.to_compact_string(), "array<string>");
    }

    #[test]
    fn analyzes_object_constructor() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "id": { "type": "number" },
                "user": {
                    "type": "object",
                    "properties": { "name": { "type": "string" } },
                    "required": ["name"],
                    "additionalProperties": false
                }
            },
            "required": ["id", "user"],
            "additionalProperties": false
        }));

        let report = check("{ id, name: .user.name }", input);
        assert_eq!(
            report.output.to_compact_string(),
            "object{id: number, name: string}"
        );
    }

    #[test]
    fn select_refines_discriminated_union() {
        let input = json_schema_to_type(&json!({
            "anyOf": [
                {
                    "type": "object",
                    "properties": {
                        "type": { "enum": ["user"] },
                        "name": { "type": "string" }
                    },
                    "required": ["type", "name"],
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "properties": {
                        "type": { "enum": ["org"] },
                        "org_name": { "type": "string" }
                    },
                    "required": ["type", "org_name"],
                    "additionalProperties": false
                }
            ]
        }));

        let report = check("select(.type == \"user\") | .name", input);
        assert_eq!(
            report.output.to_compact_string(),
            "Stream<string, ZeroOrOne>"
        );
    }

    #[test]
    fn select_refines_non_null_field() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "foo": { "type": ["string", "null"] }
            },
            "additionalProperties": false
        }));

        let report = check("select(.foo != null) | .foo", input);
        assert_eq!(
            report.output.to_compact_string(),
            "Stream<string, ZeroOrOne>"
        );
    }

    #[test]
    fn if_refines_non_null_field() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "foo": { "type": ["string", "null"] }
            },
            "additionalProperties": false
        }));

        let report = check("if .foo != null then .foo else \"missing\" end", input);
        assert_eq!(report.output.to_compact_string(), "\"missing\" | string");
    }

    #[test]
    fn select_refines_has_field() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "foo": { "type": "string" }
            },
            "additionalProperties": false
        }));

        let report = check("select(has(\"foo\")) | .foo", input);
        assert_eq!(
            report.output.to_compact_string(),
            "Stream<string, ZeroOrOne>"
        );
    }

    #[test]
    fn type_predicate_refines_unknown() {
        let report = check(
            "if type == \"array\" then [.[]] else null end",
            JType::Unknown,
        );
        assert_eq!(report.output.to_compact_string(), "array<unknown> | null");
    }

    #[test]
    fn builtins_have_useful_signatures() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "id": { "type": "number" },
                "name": { "type": "string" }
            },
            "required": ["id", "name"],
            "additionalProperties": false
        }));

        let keys = check("keys", input.clone());
        assert_eq!(keys.output.to_compact_string(), "array<\"id\" | \"name\">");

        let has = check("has(\"name\")", input.clone());
        assert_eq!(has.output.to_compact_string(), "true");

        let values = check("values", JType::union([JType::Null, JType::string()]));
        assert_eq!(
            values.output.to_compact_string(),
            "Stream<string, ZeroOrOne>"
        );

        let strings = check("strings", JType::union([JType::Null, JType::string()]));
        assert_eq!(
            strings.output.to_compact_string(),
            "Stream<string, ZeroOrOne>"
        );

        let array = JType::array(input.clone());
        let names = check("map(.name)", array);
        assert_eq!(names.output.to_compact_string(), "array<string>");

        let length = check("length", input);
        assert_eq!(length.output.to_compact_string(), "number");
    }

    #[test]
    fn variable_binding_preserves_original_dot() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "foo": { "type": "string" },
                "bar": { "type": "number" }
            },
            "required": ["foo", "bar"],
            "additionalProperties": false
        }));

        let report = check(".foo as $x | {x: $x, dot: .bar}", input);
        assert!(report.unsupported_features.is_empty());
        assert_eq!(
            report.output.to_compact_string(),
            "object{dot: number, x: string}"
        );
    }

    #[test]
    fn conversions_and_plus_support_dsl_shapes() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "params": {
                    "type": "object",
                    "properties": { "id": { "type": "string" } },
                    "required": ["id"],
                    "additionalProperties": false
                }
            },
            "required": ["params"],
            "additionalProperties": false
        }));

        let report = check(
            r#"{ id: (.params.id | tonumber), label: ("Team " + (.params.id | tostring)) }"#,
            input,
        );
        assert!(report.unsupported_features.is_empty());
        assert_eq!(
            report.output.to_compact_string(),
            "object{id: number, label: string}"
        );
    }

    #[test]
    fn assignment_updates_identity_root_paths() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "method": { "type": "string" }
            },
            "required": ["method"],
            "additionalProperties": false
        }));

        let report = check(".graphqlParams = { id: 1 }", input);
        assert!(report.unsupported_features.is_empty());
        assert_eq!(
            report.output.to_compact_string(),
            "object{graphqlParams: object{id: 1}, method: string}"
        );
    }

    #[test]
    fn collection_builtins_cover_dsl_transforms() {
        let report = check("[10, 20, 30] | add", JType::Unknown);
        assert!(report.unsupported_features.is_empty());
        assert_eq!(report.output.to_compact_string(), "null | number");

        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "keys": {
                    "type": "array",
                    "items": { "type": "number" }
                }
            },
            "required": ["keys"],
            "additionalProperties": false
        }));
        let report = check(".keys | map(tostring) | join(\",\")", input);
        assert!(report.unsupported_features.is_empty());
        assert_eq!(report.output.to_compact_string(), "string");
    }

    #[test]
    fn missing_root_property_reports_without_map_cascade() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "method": { "type": "string" },
                "params": { "type": "object", "additionalProperties": true },
                "query": { "type": "object", "additionalProperties": true }
            },
            "required": ["method", "params", "query"],
            "additionalProperties": false
        }));

        let report = check(".data.rows | map(.name)", input);
        assert_eq!(report.diagnostics.len(), 1);
        assert!(
            report.diagnostics[0]
                .message
                .contains("property \"data\" is not present on object")
        );
        assert!(!report.diagnostics[0].message.contains("map may be applied"));
        assert!(report.diagnostics[0].span.is_some());
        assert_eq!(report.output.to_compact_string(), "unknown");
    }

    #[test]
    fn map_item_missing_property_reports_on_item_shape() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "data": {
                    "type": "object",
                    "properties": {
                        "rows": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "name": { "type": "string" }
                                },
                                "required": ["id", "name"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["rows"],
                    "additionalProperties": false
                }
            },
            "required": ["data"],
            "additionalProperties": false
        }));

        let report = check(".data.rows | map(.namei)", input);
        assert_eq!(report.diagnostics.len(), 1);
        assert!(
            report.diagnostics[0]
                .message
                .contains("property \"namei\" is not present on object{id: string, name: string}")
        );
        assert!(report.diagnostics[0].span.is_some());
        assert_eq!(report.output.to_compact_string(), "array<null>");
    }

    #[test]
    fn slices_and_interpolation_are_analyzed() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "body": { "type": "string" },
                "city": { "type": "string" }
            },
            "required": ["body", "city"],
            "additionalProperties": false
        }));

        let slice = check(r#".body | .[0:50] + "...""#, input.clone());
        assert!(slice.unsupported_features.is_empty());
        assert_eq!(slice.output.to_compact_string(), "string");

        let interpolation = check(r#""Weather for \(.city)""#, input);
        assert!(interpolation.unsupported_features.is_empty());
        assert_eq!(interpolation.output.to_compact_string(), "string");
    }

    #[test]
    fn reduce_dynamic_update_groups_rows() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "data": {
                    "type": "object",
                    "properties": {
                        "rows": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "team_id": { "type": ["string", "number"] },
                                    "name": { "type": "string" }
                                },
                                "required": ["team_id"],
                                "additionalProperties": true
                            }
                        }
                    },
                    "required": ["rows"],
                    "additionalProperties": false
                }
            },
            "required": ["data"],
            "additionalProperties": false
        }));

        let report = check(
            "reduce .data.rows[] as $row ({}; .[$row.team_id | tostring] += [$row])",
            input,
        );
        assert!(report.unsupported_features.is_empty());
        let compact = report.output.to_compact_string();
        assert!(compact.contains("object{}"));
        assert!(compact.contains("...: array<object"));
        assert!(compact.contains("team_id: number | string"));
    }

    #[test]
    fn unsupported_builtin_reports_warning() {
        let report = check("group_by(.name)", JType::array(JType::Unknown));
        assert_eq!(report.output.to_compact_string(), "unknown");
        assert_eq!(report.unsupported_features.len(), 1);
        assert!(
            report.diagnostics[0]
                .message
                .contains("unsupported builtin or call `group_by`")
        );
    }
}
