use std::collections::{BTreeMap, HashMap};

use jaq_syn::filter::{AssignOp, BinaryOp, Filter as JaqFilter, FoldType, KeyVal};
use jaq_syn::path::{Opt, Part};
use jaq_syn::string;
use jaq_syn::{Arg, Def, Main};
use jaq_syn::{MathOp, OrdOp, Span, Spanned};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::diagnostic::{Diagnostic, Severity, SourceSpan};
use crate::schema::{json_schema_to_type, sample_to_type, type_to_json_schema};
use crate::stream::{Cardinality, StreamType};
use crate::types::{BoolType, JType, NumberType, Property, StringType};

type Filter = JaqFilter<String, String, String>;
type SpannedFilter = Spanned<Filter>;

const NUMERIC_ZERO_ARG_BUILTINS: &[&str] = &[
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
];

const NUMERIC_PAIR_ZERO_ARG_BUILTINS: &[&str] = &["frexp", "lgamma_r", "modf"];

const NUMERIC_PREDICATE_ZERO_ARG_BUILTINS: &[&str] =
    &["isfinite", "isinfinite", "isnan", "isnormal", "signbit"];

const NUMERIC_TWO_ARG_BUILTINS: &[&str] = &[
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
];

const NUMERIC_THREE_ARG_BUILTINS: &[&str] = &["fma"];

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

        let mut analyzer = Analyzer::new(options, source, &filter);
        let output = analyzer.analyze_main(&filter, input.into_type());
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

#[derive(Clone)]
struct FilterArgBinding {
    body: SpannedFilter,
    env: BTreeMap<String, JType>,
    filter_args: BTreeMap<String, FilterArgBinding>,
    defs: BTreeMap<String, DefEntry>,
}

#[derive(Clone)]
struct DefEntry {
    args: Vec<Arg>,
    body: Main,
    captured_env: BTreeMap<String, JType>,
    captured_filter_args: BTreeMap<String, FilterArgBinding>,
    captured_defs: BTreeMap<String, DefEntry>,
}

struct Analyzer {
    options: AnalyzeOptions,
    source_spans: AnalyzerSourceSpans,
    diagnostics: Vec<Diagnostic>,
    unsupported_features: Vec<UnsupportedFeature>,
    env: BTreeMap<String, JType>,
    filter_args: BTreeMap<String, FilterArgBinding>,
    defs: BTreeMap<String, DefEntry>,
    def_call_depth: BTreeMap<String, usize>,
    max_def_depth: usize,
    missing_path_null: bool,
    alt_lhs_depth: usize,
}

#[derive(Default)]
struct AnalyzerSourceSpans {
    filters: HashMap<usize, Span>,
    strings: HashMap<usize, Span>,
}

fn filter_key(filter: &SpannedFilter) -> usize {
    filter as *const SpannedFilter as usize
}

fn string_key(value: &jaq_syn::Str<SpannedFilter>) -> usize {
    value as *const jaq_syn::Str<SpannedFilter> as usize
}

fn collect_source_spans(source: &str, main: &Main) -> AnalyzerSourceSpans {
    let mut collector = SourceSpanCollector::new(source);
    collector.collect_main(main);
    collector.spans
}

// jaq-syn exposes spans, but for some jq path forms they can cover the whole
// source. Build a lightweight source-order overlay for diagnostics.
struct SourceSpanCollector<'a> {
    source: &'a str,
    cursor: usize,
    spans: AnalyzerSourceSpans,
}

impl<'a> SourceSpanCollector<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            cursor: 0,
            spans: AnalyzerSourceSpans::default(),
        }
    }

    fn collect_main(&mut self, main: &Main) {
        for def in &main.defs {
            self.collect_def(def);
        }
        self.visit_filter(&main.body);
    }

    fn collect_def(&mut self, def: &Def) {
        let _ = self.find_token("def");
        let _ = self.find_token(&def.lhs.name);
        for arg in &def.lhs.args {
            match arg {
                Arg::Var(name) => {
                    let _ = self.find_token(&format!("${name}"));
                }
                Arg::Fun(name) => {
                    let _ = self.find_token(name);
                }
            }
        }
        self.collect_main(&def.rhs);
    }

    fn visit_filter(&mut self, filter: &SpannedFilter) {
        match &filter.0 {
            Filter::Id | Filter::Recurse => {}
            Filter::Num(value) => {
                self.record_filter(filter, value);
            }
            Filter::Str(value) => {
                self.record_string(value);
                if let Some(span) = self.spans.strings.get(&string_key(value)).cloned() {
                    self.spans.filters.insert(filter_key(filter), span);
                }
            }
            Filter::Array(Some(inner)) => self.visit_filter(inner),
            Filter::Array(None) => {}
            Filter::Object(items) => {
                for item in items {
                    self.visit_object_item(item);
                }
            }
            Filter::Path(base, path) => {
                if !matches!(base.0, Filter::Id) || path.is_empty() {
                    self.visit_filter(base);
                }
                for (part, _) in path {
                    self.visit_path_part(part);
                }
            }
            Filter::Binary(left, op, right) => {
                self.visit_filter(left);
                let _ = self.find_token(binary_op_token(op));
                self.visit_filter(right);
            }
            Filter::Ite(branches, else_branch) => {
                for (condition, then_branch) in branches {
                    self.visit_filter(condition);
                    self.visit_filter(then_branch);
                }
                if let Some(else_branch) = else_branch {
                    self.visit_filter(else_branch);
                }
            }
            Filter::Call(name, args) => {
                self.record_filter(filter, name);
                for arg in args {
                    self.visit_filter(arg);
                }
            }
            Filter::Try(inner) => self.visit_filter(inner),
            Filter::TryCatch(inner, catch) => {
                self.visit_filter(inner);
                if let Some(catch) = catch {
                    self.visit_filter(catch);
                }
            }
            Filter::Neg(inner) => {
                let _ = self.find_token("-");
                self.visit_filter(inner);
            }
            Filter::Var(name) => {
                self.record_filter(filter, &format!("${name}"));
            }
            Filter::Fold(_, fold) => {
                self.visit_filter(&fold.xs);
                let _ = self.find_token(&format!("${}", fold.x));
                self.visit_filter(&fold.init);
                self.visit_filter(&fold.f);
            }
        }
    }

    fn visit_object_item(&mut self, item: &KeyVal<SpannedFilter>) {
        match item {
            KeyVal::Filter(key_filter, value_filter) => {
                self.visit_filter(key_filter);
                self.visit_filter(value_filter);
            }
            KeyVal::Str(key, value_filter) => {
                self.record_string(key);
                if let Some(value_filter) = value_filter {
                    self.visit_filter(value_filter);
                }
            }
        }
    }

    fn visit_path_part(&mut self, part: &Part<SpannedFilter>) {
        match part {
            Part::Index(index) => {
                if let Some(key) = literal_string_filter(index) {
                    let span = self.find_property(&key);
                    if let Some(span) = span {
                        self.spans.filters.insert(filter_key(index), span);
                    }
                } else {
                    self.visit_filter(index);
                }
            }
            Part::Range(from, to) => {
                if let Some(from) = from {
                    self.visit_filter(from);
                }
                if let Some(to) = to {
                    self.visit_filter(to);
                }
            }
        }
    }

    fn record_filter(&mut self, filter: &SpannedFilter, token: &str) {
        if let Some(span) = self.find_token(token) {
            self.spans.filters.insert(filter_key(filter), span);
        }
    }

    fn record_string(&mut self, value: &jaq_syn::Str<SpannedFilter>) {
        if let Some(literal) = literal_string(value) {
            if let Some(span) = self.find_string_literal(&literal) {
                self.spans.strings.insert(string_key(value), span);
            }
            return;
        }

        let _ = self.find_token("\"");
        for part in &value.parts {
            if let string::Part::Fun(filter) = part {
                self.visit_filter(filter);
            }
        }
    }

    fn find_property(&mut self, key: &str) -> Option<Span> {
        self.find_token(&format!(".{key}"))
            .or_else(|| self.find_token(key))
    }

    fn find_string_literal(&mut self, value: &str) -> Option<Span> {
        let token = serde_json::to_string(value).ok()?;
        let start = self.source.get(self.cursor..)?.find(&token)? + self.cursor;
        self.cursor = start + token.len();
        if token.len() >= 2 {
            Some(start + 1..start + token.len() - 1)
        } else {
            Some(start..start + token.len())
        }
    }

    fn find_token(&mut self, token: &str) -> Option<Span> {
        if token.is_empty() {
            return None;
        }
        let start = self.source.get(self.cursor..)?.find(token)? + self.cursor;
        self.cursor = start + token.len();
        Some(start..self.cursor)
    }
}

fn binary_op_token(op: &BinaryOp) -> &'static str {
    match op {
        BinaryOp::Pipe(_) => "|",
        BinaryOp::Comma => ",",
        BinaryOp::Alt => "//",
        BinaryOp::Or => "or",
        BinaryOp::And => "and",
        BinaryOp::Math(MathOp::Add) => "+",
        BinaryOp::Math(MathOp::Sub) => "-",
        BinaryOp::Math(MathOp::Mul) => "*",
        BinaryOp::Math(MathOp::Div) => "/",
        BinaryOp::Math(MathOp::Rem) => "%",
        BinaryOp::Assign(AssignOp::Assign) => "=",
        BinaryOp::Assign(AssignOp::Update) => "|=",
        BinaryOp::Assign(AssignOp::UpdateWith(MathOp::Add)) => "+=",
        BinaryOp::Assign(AssignOp::UpdateWith(MathOp::Sub)) => "-=",
        BinaryOp::Assign(AssignOp::UpdateWith(MathOp::Mul)) => "*=",
        BinaryOp::Assign(AssignOp::UpdateWith(MathOp::Div)) => "/=",
        BinaryOp::Assign(AssignOp::UpdateWith(MathOp::Rem)) => "%=",
        BinaryOp::Ord(OrdOp::Eq) => "==",
        BinaryOp::Ord(OrdOp::Ne) => "!=",
        BinaryOp::Ord(OrdOp::Lt) => "<",
        BinaryOp::Ord(OrdOp::Le) => "<=",
        BinaryOp::Ord(OrdOp::Gt) => ">",
        BinaryOp::Ord(OrdOp::Ge) => ">=",
    }
}

#[derive(Clone, Debug)]
struct PredicateRefinement {
    when_true: JType,
    when_false: JType,
}

impl Analyzer {
    fn new(options: AnalyzeOptions, source: &str, main: &Main) -> Self {
        Self {
            source_spans: collect_source_spans(source, main),
            options,
            diagnostics: Vec::new(),
            unsupported_features: Vec::new(),
            env: BTreeMap::new(),
            filter_args: BTreeMap::new(),
            defs: BTreeMap::new(),
            def_call_depth: BTreeMap::new(),
            max_def_depth: 4,
            missing_path_null: false,
            alt_lhs_depth: 0,
        }
    }

    fn span_for_filter(&self, filter: &SpannedFilter) -> Span {
        self.source_spans
            .filters
            .get(&filter_key(filter))
            .cloned()
            .unwrap_or_else(|| filter.1.clone())
    }

    fn span_for_string(&self, value: &jaq_syn::Str<SpannedFilter>, fallback: Span) -> Span {
        self.source_spans
            .strings
            .get(&string_key(value))
            .cloned()
            .unwrap_or(fallback)
    }

    fn analyze_main(&mut self, main: &Main, input: JType) -> StreamType {
        let saved_defs = self.defs.clone();
        for def in &main.defs {
            self.register_def(def);
        }
        let out = self.analyze(&main.body, input);
        self.defs = saved_defs;
        out
    }

    fn register_def(&mut self, def: &Def) {
        let key = format!("{}/{}", def.lhs.name, def.lhs.args.len());
        let entry = DefEntry {
            args: def.lhs.args.clone(),
            body: def.rhs.clone(),
            captured_env: self.env.clone(),
            captured_filter_args: self.filter_args.clone(),
            captured_defs: self.defs.clone(),
        };
        // Register the def so its body can see itself (basic recursion)
        let mut captured_defs = entry.captured_defs.clone();
        captured_defs.insert(key.clone(), entry.clone());
        let entry = DefEntry {
            captured_defs,
            ..entry
        };
        self.defs.insert(key, entry);
    }

    fn call_def(
        &mut self,
        entry: DefEntry,
        actual_args: &[SpannedFilter],
        input: JType,
    ) -> StreamType {
        let arity_key = format!("/{}/{}", entry.args.len(), "_call");
        let depth = self.def_call_depth.get(&arity_key).copied().unwrap_or(0);
        if depth >= self.max_def_depth {
            return StreamType::one(JType::Unknown);
        }
        self.def_call_depth.insert(arity_key.clone(), depth + 1);

        let saved_env = std::mem::replace(&mut self.env, entry.captured_env.clone());
        let saved_filter_args =
            std::mem::replace(&mut self.filter_args, entry.captured_filter_args.clone());
        let saved_defs = std::mem::replace(&mut self.defs, entry.captured_defs.clone());

        for (i, formal) in entry.args.iter().enumerate() {
            let actual = &actual_args[i];
            match formal {
                Arg::Var(name) => {
                    // Evaluate against the call-site input using *caller* scope
                    let caller_env = saved_env.clone();
                    let caller_filter_args = saved_filter_args.clone();
                    let caller_defs = saved_defs.clone();
                    let temp_env = std::mem::replace(&mut self.env, caller_env);
                    let temp_filter_args =
                        std::mem::replace(&mut self.filter_args, caller_filter_args);
                    let temp_defs = std::mem::replace(&mut self.defs, caller_defs);
                    let bound = self.analyze(actual, input.clone()).item;
                    self.env = temp_env;
                    self.filter_args = temp_filter_args;
                    self.defs = temp_defs;
                    self.env.insert(name.clone(), bound);
                }
                Arg::Fun(name) => {
                    self.filter_args.insert(
                        name.clone(),
                        FilterArgBinding {
                            body: actual.clone(),
                            env: saved_env.clone(),
                            filter_args: saved_filter_args.clone(),
                            defs: saved_defs.clone(),
                        },
                    );
                }
            }
        }

        let out = self.analyze_main(&entry.body, input);

        self.env = saved_env;
        self.filter_args = saved_filter_args;
        self.defs = saved_defs;
        self.def_call_depth.insert(arity_key, depth);
        out
    }

    fn invoke_filter_arg(&mut self, binding: FilterArgBinding, input: JType) -> StreamType {
        let saved_env = std::mem::replace(&mut self.env, binding.env);
        let saved_filter_args = std::mem::replace(&mut self.filter_args, binding.filter_args);
        let saved_defs = std::mem::replace(&mut self.defs, binding.defs);
        let out = self.analyze(&binding.body, input);
        self.env = saved_env;
        self.filter_args = saved_filter_args;
        self.defs = saved_defs;
        out
    }

    fn analyze(&mut self, filter: &SpannedFilter, input: JType) -> StreamType {
        let span = self.span_for_filter(filter);
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
                let _ = span;
                StreamType::zero_or_more(input.recursive_descent())
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
                KeyVal::Str(key_filter, value_filter) => {
                    if let Some(key) = literal_string(key_filter) {
                        let span = self.span_for_string(key_filter, 0..0);
                        let value = match value_filter {
                            Some(value_filter) => self.analyze(value_filter, input.clone()).item,
                            None => self.access_field(input.clone(), &key, false, span).item,
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
                let span = self.span_for_filter(index);
                if let Some(key) = literal_string_filter(index) {
                    self.access_field(input, &key, matches!(opt, Opt::Optional), span)
                } else if let Some(index) = literal_i64_filter(index) {
                    self.access_index(input, index, matches!(opt, Opt::Optional), span)
                } else {
                    self.access_dynamic_index(input, matches!(opt, Opt::Optional), span)
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
            BinaryOp::Ord(_) | BinaryOp::Or | BinaryOp::And => {
                self.analyze_boolean_operands(left, right, input);
                StreamType::one(JType::bool())
            }
            BinaryOp::Math(op) => {
                let left = self.analyze(left, input.clone());
                let right = self.analyze(right, input);
                StreamType::new(
                    math_type(*op, left.item, right.item),
                    left.card.compose(right.card),
                )
            }
            BinaryOp::Alt => {
                self.alt_lhs_depth += 1;
                let left = self.analyze(left, input.clone());
                self.alt_lhs_depth -= 1;
                let right = self.analyze(right, input);
                StreamType::new(
                    alt_type(left.item, right.item),
                    left.card.alternative(right.card),
                )
            }
            BinaryOp::Assign(op) => self.analyze_assignment(left, op, right, input),
        }
    }

    fn analyze_boolean_operands(
        &mut self,
        left: &SpannedFilter,
        right: &SpannedFilter,
        input: JType,
    ) {
        self.with_fresh_missing_path_scope(|analyzer| {
            let _ = analyzer.analyze(left, input.clone());
            let _ = analyzer.analyze(right, input);
        });
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
        // User-defined function lookup
        let key = format!("{name}/{}", args.len());
        if let Some(entry) = self.defs.get(&key).cloned() {
            return self.call_def(entry, args, input);
        }
        // Filter-arg lookup (no actual args at call site)
        if args.is_empty()
            && let Some(binding) = self.filter_args.get(name).cloned()
        {
            return self.invoke_filter_arg(binding, input);
        }
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
            (name, []) if NUMERIC_ZERO_ARG_BUILTINS.contains(&name) => {
                StreamType::one(JType::number())
            }
            (name, []) if NUMERIC_PAIR_ZERO_ARG_BUILTINS.contains(&name) => {
                StreamType::one(JType::array(JType::number()))
            }
            (name, []) if NUMERIC_PREDICATE_ZERO_ARG_BUILTINS.contains(&name) => {
                StreamType::one(JType::bool())
            }
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
            ("flatten", []) => StreamType::one(self.flatten_type(input, FlattenDepth::Full)),
            ("flatten", [depth]) => {
                let depth_ty = self.analyze(depth, input.clone()).item;
                StreamType::one(self.flatten_type(input, flatten_depth_from_type(&depth_ty)))
            }
            ("range", [_]) | ("range", [_, _]) | ("range", [_, _, _]) => {
                for arg in args {
                    let _ = self.analyze(arg, input.clone());
                }
                StreamType::zero_or_more(JType::number())
            }
            (name, [_, _]) if NUMERIC_TWO_ARG_BUILTINS.contains(&name) => {
                for arg in args {
                    let _ = self.analyze(arg, input.clone());
                }
                StreamType::one(JType::number())
            }
            (name, [_, _, _]) if NUMERIC_THREE_ARG_BUILTINS.contains(&name) => {
                for arg in args {
                    let _ = self.analyze(arg, input.clone());
                }
                StreamType::one(JType::number())
            }
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
            ("iterables", []) => StreamType::zero_or_one(JType::union([
                self.filter_kind(input.clone(), "array").item,
                self.filter_kind(input, "object").item,
            ])),
            ("scalars", []) => StreamType::zero_or_one(JType::union([
                self.filter_kind(input.clone(), "null").item,
                self.filter_kind(input.clone(), "boolean").item,
                self.filter_kind(input.clone(), "number").item,
                self.filter_kind(input, "string").item,
            ])),
            ("leaf_paths", []) | ("paths", []) | ("paths", [_]) => {
                for arg in args {
                    let _ = self.analyze(arg, input.clone());
                }
                StreamType::zero_or_more(JType::array(JType::union([
                    JType::string(),
                    JType::number(),
                ])))
            }
            ("path", [inner]) => {
                let _ = self.analyze(inner, input);
                StreamType::one(JType::array(JType::union([
                    JType::string(),
                    JType::number(),
                ])))
            }
            ("getpath", [path]) => {
                let _ = self.analyze(path, input);
                StreamType::one(JType::Unknown)
            }
            ("setpath", [path, value]) => {
                let _ = self.analyze(path, input.clone());
                let value = self.analyze(value, input.clone()).item;
                StreamType::one(self.write_dynamic_object_key(input, value, span))
            }
            ("del", [_]) | ("delpaths", [_]) => {
                for arg in args {
                    let _ = self.analyze(arg, input.clone());
                }
                StreamType::one(del_type(input))
            }
            ("walk", [f]) => StreamType::one(self.walk_type(f, input)),
            ("sort", []) | ("sort_by", [_]) | ("unique", []) | ("unique_by", [_]) => {
                for arg in args {
                    let _ = self.analyze(arg, input.clone());
                }
                StreamType::one(self.same_array_type(input))
            }
            ("group_by", [grouper]) => {
                let _ = self.analyze(grouper, input.clone());
                StreamType::one(self.group_by_type(input))
            }
            ("min", []) | ("max", []) | ("min_by", [_]) | ("max_by", [_]) => {
                for arg in args {
                    let _ = self.analyze(arg, input.clone());
                }
                StreamType::one(self.array_item_or_null(input))
            }
            ("reverse", []) => StreamType::one(self.same_array_or_string(input)),
            ("any", []) | ("all", []) => StreamType::one(JType::bool()),
            ("any", [cond]) | ("all", [cond]) => {
                let _ = self.analyze(cond, input);
                StreamType::one(JType::bool())
            }
            ("any", [generator, cond]) | ("all", [generator, cond]) => {
                let _ = self.analyze(generator, input.clone());
                let _ = self.analyze(cond, input);
                StreamType::one(JType::bool())
            }
            ("contains", [arg]) | ("inside", [arg]) => {
                let _ = self.analyze(arg, input);
                StreamType::one(JType::bool())
            }
            ("index", [arg]) => {
                let _ = self.analyze(arg, input);
                StreamType::one(JType::union([JType::number(), JType::Null]))
            }
            ("startswith", [arg]) | ("endswith", [arg]) => {
                let _ = self.analyze(arg, input);
                StreamType::one(JType::bool())
            }
            ("test", [re]) | ("test", [re, _]) => {
                for arg in args {
                    let _ = self.analyze(arg, input.clone());
                }
                let _ = re;
                StreamType::one(JType::bool())
            }
            ("ltrimstr", [arg]) | ("rtrimstr", [arg]) => {
                let _ = self.analyze(arg, input.clone());
                StreamType::one(trim_str_type(input))
            }
            ("ascii_downcase", []) => StreamType::one(ascii_downcase_type(input)),
            ("tojson", []) => StreamType::one(JType::string()),
            ("fromjson", []) => StreamType::one(JType::Unknown),
            ("split", [arg]) => {
                let _ = self.analyze(arg, input);
                StreamType::one(JType::array(JType::string()))
            }
            ("split", [re, flags]) => {
                let _ = self.analyze(re, input.clone());
                let _ = self.analyze(flags, input);
                StreamType::one(JType::array(JType::string()))
            }
            ("splits", [_]) | ("splits", [_, _]) => {
                for arg in args {
                    let _ = self.analyze(arg, input.clone());
                }
                StreamType::zero_or_more(JType::string())
            }
            ("explode", []) => StreamType::one(JType::array(JType::number())),
            ("implode", []) => StreamType::one(JType::string()),
            ("ascii", []) => StreamType::one(JType::string()),
            ("tojson", [_]) => {
                for arg in args {
                    let _ = self.analyze(arg, input.clone());
                }
                StreamType::one(JType::string())
            }
            ("env", []) => StreamType::one(JType::object(BTreeMap::new(), Some(JType::string()))),
            ("$__loc__", []) | ("input_line_number", []) => StreamType::one(JType::Unknown),
            ("error", []) => StreamType::zero(),
            ("error", [arg]) => {
                let _ = self.analyze(arg, input);
                StreamType::zero()
            }
            ("@text", [])
            | ("@json", [])
            | ("@html", [])
            | ("@uri", [])
            | ("@csv", [])
            | ("@tsv", [])
            | ("@sh", [])
            | ("@base32", [])
            | ("@base32d", [])
            | ("@base64", [])
            | ("@base64d", []) => StreamType::one(JType::string()),
            ("debug", []) => StreamType::one(input),
            ("debug", [arg]) => {
                let _ = self.analyze(arg, input.clone());
                StreamType::one(input)
            }
            ("stderr", []) => StreamType::one(input),
            ("to_entries", []) => StreamType::one(self.entries_of_type(input)),
            ("from_entries", []) => StreamType::one(self.object_of_entries_type(input)),
            ("with_entries", [f]) => StreamType::one(self.with_entries_type(f, input)),
            ("match", [re]) | ("match", [re, _]) => {
                for arg in args {
                    let _ = self.analyze(arg, input.clone());
                }
                let _ = re;
                StreamType::zero_or_more(regex_match_type())
            }
            ("capture", [re]) | ("capture", [re, _]) => {
                for arg in args {
                    let _ = self.analyze(arg, input.clone());
                }
                let _ = re;
                StreamType::zero_or_more(JType::object(
                    BTreeMap::new(),
                    Some(JType::union([JType::string(), JType::Null])),
                ))
            }
            ("scan", [_]) | ("scan", [_, _]) => {
                for arg in args {
                    let _ = self.analyze(arg, input.clone());
                }
                StreamType::zero_or_more(JType::union([
                    JType::string(),
                    JType::array(JType::string()),
                ]))
            }
            ("gsub", _) | ("sub", _) => {
                for arg in args {
                    let _ = self.analyze(arg, input.clone());
                }
                StreamType::one(JType::string())
            }
            ("limit", [n, generator]) => {
                let _ = self.analyze(n, input.clone());
                let stream = self.analyze(generator, input);
                StreamType::zero_or_more(stream.item)
            }
            ("first", []) => StreamType::one(self.first_type(input)),
            ("first", [generator]) => {
                let stream = self.analyze(generator, input);
                StreamType::zero_or_one(stream.item)
            }
            ("last", [generator]) => {
                let stream = self.analyze(generator, input);
                StreamType::zero_or_one(stream.item)
            }
            ("nth", [_, generator]) => {
                let _ = self.analyze(&args[0], input.clone());
                let stream = self.analyze(generator, input);
                StreamType::zero_or_one(stream.item)
            }
            ("isempty", [generator]) => {
                let _ = self.analyze(generator, input);
                StreamType::one(JType::bool())
            }
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
                    self.analyze_boolean_operands(left, right, input.clone());
                    return self.refine_type_predicate(input, &kind, *op);
                }
                if let Some(kind) = type_comparison_kind(right, left) {
                    self.analyze_boolean_operands(left, right, input.clone());
                    return self.refine_type_predicate(input, &kind, *op);
                }
                if let (Some(field), Some(literal)) =
                    (top_level_field_access(left), literal_type_filter(right))
                {
                    self.analyze_boolean_operands(left, right, input.clone());
                    return self.refine_field_literal_predicate(input, &field, literal, *op);
                }
                if let (Some(field), Some(literal)) =
                    (top_level_field_access(right), literal_type_filter(left))
                {
                    self.analyze_boolean_operands(left, right, input.clone());
                    return self.refine_field_literal_predicate(input, &field, literal, *op);
                }
                if let Some((field, kind)) = recognize_length_predicate(left, right, *op)
                    .or_else(|| recognize_length_predicate(right, left, flip_ord(*op)))
                {
                    return refine_field_emptiness(input, &field, kind);
                }
            }
            Filter::Binary(
                left,
                BinaryOp::Ord(op @ (OrdOp::Gt | OrdOp::Ge | OrdOp::Lt | OrdOp::Le)),
                right,
            ) => {
                if let Some((field, kind)) = recognize_length_predicate(left, right, *op)
                    .or_else(|| recognize_length_predicate(right, left, flip_ord(*op)))
                {
                    return refine_field_emptiness(input, &field, kind);
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

    fn flatten_type(&mut self, input: JType, depth: FlattenDepth) -> JType {
        match input {
            JType::Array(array) => {
                self.missing_path_null = false;
                JType::array(flatten_item(*array.items, depth))
            }
            JType::Union(items) => {
                JType::union(items.into_iter().map(|item| self.flatten_type(item, depth)))
            }
            JType::Null if self.missing_path_null => {
                self.missing_path_null = false;
                JType::Unknown
            }
            JType::Unknown => {
                self.missing_path_null = false;
                JType::array(JType::Unknown)
            }
            other => {
                self.warn_or_error(
                    format!(
                        "flatten may be applied to non-array input: {}",
                        other.to_compact_string()
                    ),
                    None,
                );
                JType::Unknown
            }
        }
    }

    fn same_array_type(&mut self, input: JType) -> JType {
        match input {
            JType::Array(_) => input,
            JType::Union(items) => {
                JType::union(items.into_iter().map(|item| self.same_array_type(item)))
            }
            JType::Null if self.missing_path_null => {
                self.missing_path_null = false;
                JType::Unknown
            }
            JType::Unknown => JType::array(JType::Unknown),
            other => {
                self.warn_or_error(
                    format!("expected array input, got: {}", other.to_compact_string()),
                    None,
                );
                JType::Unknown
            }
        }
    }

    fn same_array_or_string(&mut self, input: JType) -> JType {
        match input {
            JType::Array(_) | JType::String(_) => input,
            JType::Union(items) => JType::union(
                items
                    .into_iter()
                    .map(|item| self.same_array_or_string(item)),
            ),
            JType::Unknown => JType::Unknown,
            other => {
                self.warn_or_error(
                    format!(
                        "expected array or string input, got: {}",
                        other.to_compact_string()
                    ),
                    None,
                );
                JType::Unknown
            }
        }
    }

    fn array_item_or_null(&mut self, input: JType) -> JType {
        match input {
            JType::Array(array) => JType::union([*array.items, JType::Null]),
            JType::Union(items) => {
                JType::union(items.into_iter().map(|item| self.array_item_or_null(item)))
            }
            JType::Unknown => JType::Unknown,
            JType::Null => JType::Null,
            other => {
                self.warn_or_error(
                    format!("expected array input, got: {}", other.to_compact_string()),
                    None,
                );
                JType::Unknown
            }
        }
    }

    fn first_type(&mut self, input: JType) -> JType {
        match input {
            JType::Array(array) => {
                self.missing_path_null = false;
                JType::union([*array.items, JType::Null])
            }
            JType::Null => {
                self.missing_path_null = false;
                JType::Null
            }
            JType::Union(items) => {
                JType::union(items.into_iter().map(|item| self.first_type(item)))
            }
            JType::Unknown => {
                self.missing_path_null = false;
                JType::Unknown
            }
            other => {
                self.missing_path_null = false;
                self.warn_or_error(
                    format!("expected array input, got: {}", other.to_compact_string()),
                    None,
                );
                JType::Unknown
            }
        }
    }

    fn group_by_type(&mut self, input: JType) -> JType {
        match input {
            JType::Array(array) => JType::array(JType::array(*array.items)),
            JType::Union(items) => {
                JType::union(items.into_iter().map(|item| self.group_by_type(item)))
            }
            JType::Unknown => JType::array(JType::array(JType::Unknown)),
            other => {
                self.warn_or_error(
                    format!(
                        "group_by expected array input, got: {}",
                        other.to_compact_string()
                    ),
                    None,
                );
                JType::Unknown
            }
        }
    }

    fn walk_type(&mut self, f: &SpannedFilter, input: JType) -> JType {
        let descended = input.recursive_descent();
        let mapped = self.analyze(f, descended).item;
        JType::union([input, mapped])
    }

    fn entries_of_type(&mut self, input: JType) -> JType {
        let value_ty = match &input {
            JType::Object(object) => {
                let mut values = object
                    .properties
                    .values()
                    .map(|prop| prop.ty.clone())
                    .collect::<Vec<_>>();
                if let Some(additional) = &object.additional {
                    values.push((**additional).clone());
                }
                JType::union(values)
            }
            JType::Unknown => JType::Unknown,
            JType::Union(items) => {
                return JType::union(items.iter().map(|item| self.entries_of_type(item.clone())));
            }
            other => {
                self.warn_or_error(
                    format!(
                        "to_entries expected object input, got: {}",
                        other.to_compact_string()
                    ),
                    None,
                );
                return JType::Unknown;
            }
        };
        let mut entry_props = BTreeMap::new();
        entry_props.insert(
            "key".to_string(),
            Property {
                ty: JType::string(),
                required: true,
            },
        );
        entry_props.insert(
            "value".to_string(),
            Property {
                ty: value_ty,
                required: true,
            },
        );
        JType::array(JType::closed_object(entry_props))
    }

    fn object_of_entries_type(&mut self, input: JType) -> JType {
        match &input {
            JType::Array(array) => match &*array.items {
                JType::Object(object) => {
                    let value_ty = object
                        .properties
                        .get("value")
                        .map(|prop| prop.ty.clone())
                        .or_else(|| object.properties.get("v").map(|prop| prop.ty.clone()))
                        .unwrap_or(JType::Unknown);
                    JType::object(BTreeMap::new(), Some(value_ty))
                }
                JType::Unknown => JType::object(BTreeMap::new(), Some(JType::Unknown)),
                _ => JType::object(BTreeMap::new(), Some(JType::Unknown)),
            },
            JType::Unknown => JType::object(BTreeMap::new(), Some(JType::Unknown)),
            JType::Union(items) => JType::union(
                items
                    .iter()
                    .map(|item| self.object_of_entries_type(item.clone())),
            ),
            other => {
                self.warn_or_error(
                    format!(
                        "from_entries expected array input, got: {}",
                        other.to_compact_string()
                    ),
                    None,
                );
                JType::Unknown
            }
        }
    }

    fn with_entries_type(&mut self, f: &SpannedFilter, input: JType) -> JType {
        let entries = self.entries_of_type(input);
        let mapped = match entries.clone() {
            JType::Array(array) => self.analyze(f, *array.items).item,
            other => self.analyze(f, other).item,
        };
        let arr_of_mapped = JType::array(mapped);
        self.object_of_entries_type(arr_of_mapped)
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
                    } else if self.alt_lhs_depth > 0 {
                        self.missing_path_null = true;
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
                let span = self.span_for_filter(index);
                if let Some(key) = literal_string_filter(index) {
                    self.write_field(input, &key, rest, value, span)
                } else if literal_i64_filter(index).is_some() {
                    self.write_array_index(input, rest, value, span)
                } else {
                    let key_type = self.analyze(index, input.clone()).item;
                    self.write_dynamic_index(input, key_type, rest, value, span)
                }
            }
            Part::Range(None, None) => self.write_array_index(input, rest, value, span),
            Part::Range(_, _) => self.write_slice(input, rest, value, span),
        }
    }

    fn write_slice(
        &mut self,
        input: JType,
        rest: &[(Part<SpannedFilter>, Opt)],
        value: JType,
        span: Span,
    ) -> JType {
        let inner_seed = match &value {
            JType::Array(array) => (*array.items).clone(),
            _ => JType::Unknown,
        };
        let written_inner = self.write_path_parts(inner_seed, rest, value.clone(), span.clone());
        match input {
            JType::Array(existing) => JType::array(JType::union([*existing.items, written_inner])),
            JType::Null | JType::Unknown => JType::array(written_inner),
            JType::Union(items) => JType::union(
                items
                    .into_iter()
                    .map(|item| self.write_slice(item, rest, value.clone(), span.clone())),
            ),
            other => {
                self.warn_or_error(
                    format!(
                        "slice assignment may be applied to non-array input: {}",
                        other.to_compact_string()
                    ),
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
        if let Some(keys) = finite_string_literals(&key_type) {
            let mut out = input;
            for key in keys {
                out = self.write_field(out, &key, rest, value.clone(), span.clone());
            }
            return out;
        }

        if is_string_like(&key_type) {
            return self.write_dynamic_object_key_with_rest(input, rest, value, span);
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
                    self.write_dynamic_object_key_with_rest(other, rest, value, span),
                    JType::Unknown,
                ])
            }
        }
    }

    fn write_dynamic_object_key_with_rest(
        &mut self,
        input: JType,
        rest: &[(Part<SpannedFilter>, Opt)],
        value: JType,
        span: Span,
    ) -> JType {
        let written_inner = self.write_path_parts(JType::Unknown, rest, value, span.clone());
        self.write_dynamic_object_key(input, written_inner, span)
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

fn trim_str_type(input: JType) -> JType {
    match input {
        JType::String(_) => JType::string(),
        JType::Union(items) => JType::union(items.into_iter().map(trim_str_type)),
        JType::Unknown => JType::Unknown,
        other => other,
    }
}

fn del_type(input: JType) -> JType {
    match input {
        JType::Object(mut object) => {
            for prop in object.properties.values_mut() {
                prop.required = false;
            }
            if object.additional.is_none() {
                object.additional = Some(Box::new(JType::Unknown));
            }
            JType::Object(object)
        }
        JType::Array(array) => JType::array(*array.items),
        JType::Union(items) => JType::union(items.into_iter().map(del_type)),
        JType::Unknown => JType::Unknown,
        other => other,
    }
}

fn ascii_downcase_type(input: JType) -> JType {
    match input {
        JType::Union(items) => JType::union(items.into_iter().map(ascii_downcase_type)),
        JType::String(StringType::Literal(value)) => JType::string_lit(value.to_ascii_lowercase()),
        JType::Never => JType::Never,
        _ => JType::string(),
    }
}

fn regex_match_type() -> JType {
    let mut capture_props = BTreeMap::new();
    capture_props.insert(
        "offset".to_string(),
        Property {
            ty: JType::number(),
            required: true,
        },
    );
    capture_props.insert(
        "length".to_string(),
        Property {
            ty: JType::number(),
            required: true,
        },
    );
    capture_props.insert(
        "string".to_string(),
        Property {
            ty: JType::union([JType::string(), JType::Null]),
            required: true,
        },
    );
    capture_props.insert(
        "name".to_string(),
        Property {
            ty: JType::union([JType::string(), JType::Null]),
            required: true,
        },
    );

    let mut props = BTreeMap::new();
    props.insert(
        "offset".to_string(),
        Property {
            ty: JType::number(),
            required: true,
        },
    );
    props.insert(
        "length".to_string(),
        Property {
            ty: JType::number(),
            required: true,
        },
    );
    props.insert(
        "string".to_string(),
        Property {
            ty: JType::string(),
            required: true,
        },
    );
    props.insert(
        "captures".to_string(),
        Property {
            ty: JType::array(JType::closed_object(capture_props)),
            required: true,
        },
    );
    JType::closed_object(props)
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

#[derive(Clone, Copy)]
enum LengthKind {
    Empty,
    NonEmpty,
}

#[derive(Clone, Copy)]
enum Emptiness {
    Empty,
    NonEmpty,
    Unknown,
}

#[derive(Clone, Copy)]
enum PartitionClass {
    EmptyOnly,
    NonEmptyOnly,
    Both,
}

fn is_length_call(filter: &SpannedFilter) -> bool {
    matches!(&filter.0, Filter::Call(name, args) if name == "length" && args.is_empty())
}

fn is_empty_default_literal(filter: &SpannedFilter) -> bool {
    match &filter.0 {
        Filter::Array(None) => true,
        Filter::Object(items) if items.is_empty() => true,
        Filter::Str(value) => matches!(literal_string(value), Some(s) if s.is_empty()),
        _ => false,
    }
}

fn length_of_field_pattern(filter: &SpannedFilter) -> Option<String> {
    let Filter::Binary(left, BinaryOp::Pipe(None), right) = &filter.0 else {
        return None;
    };
    if !is_length_call(right) {
        return None;
    }
    if let Some(field) = top_level_field_access(left) {
        return Some(field);
    }
    let Filter::Binary(alt_left, BinaryOp::Alt, alt_right) = &left.0 else {
        return None;
    };
    let field = top_level_field_access(alt_left)?;
    if !is_empty_default_literal(alt_right) {
        return None;
    }
    Some(field)
}

fn length_predicate_kind(op: OrdOp, literal: i64) -> Option<LengthKind> {
    match (op, literal) {
        (OrdOp::Eq, 0) => Some(LengthKind::Empty),
        (OrdOp::Ne, 0) => Some(LengthKind::NonEmpty),
        (OrdOp::Gt, 0) => Some(LengthKind::NonEmpty),
        (OrdOp::Ge, 1) => Some(LengthKind::NonEmpty),
        (OrdOp::Lt, 1) => Some(LengthKind::Empty),
        (OrdOp::Le, 0) => Some(LengthKind::Empty),
        _ => None,
    }
}

fn flip_ord(op: OrdOp) -> OrdOp {
    match op {
        OrdOp::Gt => OrdOp::Lt,
        OrdOp::Ge => OrdOp::Le,
        OrdOp::Lt => OrdOp::Gt,
        OrdOp::Le => OrdOp::Ge,
        OrdOp::Eq => OrdOp::Eq,
        OrdOp::Ne => OrdOp::Ne,
    }
}

fn recognize_length_predicate(
    left: &SpannedFilter,
    right: &SpannedFilter,
    op: OrdOp,
) -> Option<(String, LengthKind)> {
    let field = length_of_field_pattern(left)?;
    let lit = literal_i64_filter(right)?;
    let kind = length_predicate_kind(op, lit)?;
    Some((field, kind))
}

fn is_field_definitely_missing(member: &JType, field: &str) -> bool {
    matches!(member, JType::Object(obj)
        if !obj.properties.contains_key(field) && obj.additional.is_none())
}

fn classify_value_emptiness(ty: &JType) -> Emptiness {
    match ty {
        JType::Null => Emptiness::Empty,
        JType::Array(arr) if matches!(*arr.items, JType::Never) => Emptiness::Empty,
        JType::String(StringType::Literal(s)) if s.is_empty() => Emptiness::Empty,
        JType::String(StringType::Literal(_)) => Emptiness::NonEmpty,
        JType::Object(obj) if obj.properties.is_empty() && obj.additional.is_none() => {
            Emptiness::Empty
        }
        _ => Emptiness::Unknown,
    }
}

fn classify_for_partition(member: &JType, field: &str, any_missing: bool) -> PartitionClass {
    let JType::Object(obj) = member else {
        return PartitionClass::Both;
    };
    match obj.properties.get(field) {
        None => {
            if obj.additional.is_some() {
                PartitionClass::Both
            } else {
                PartitionClass::EmptyOnly
            }
        }
        Some(prop) => {
            if !prop.required {
                return PartitionClass::Both;
            }
            match classify_value_emptiness(&prop.ty) {
                Emptiness::Empty => PartitionClass::EmptyOnly,
                Emptiness::NonEmpty => PartitionClass::NonEmptyOnly,
                Emptiness::Unknown => {
                    if any_missing {
                        PartitionClass::NonEmptyOnly
                    } else {
                        PartitionClass::Both
                    }
                }
            }
        }
    }
}

fn refine_field_emptiness(input: JType, field: &str, true_kind: LengthKind) -> PredicateRefinement {
    let members = flatten_union(input);
    let any_missing = members
        .iter()
        .any(|m| is_field_definitely_missing(m, field));

    let mut empties = Vec::new();
    let mut non_empties = Vec::new();
    for member in members {
        match classify_for_partition(&member, field, any_missing) {
            PartitionClass::EmptyOnly => empties.push(member),
            PartitionClass::NonEmptyOnly => non_empties.push(member),
            PartitionClass::Both => {
                empties.push(member.clone());
                non_empties.push(member);
            }
        }
    }

    let when_empty = JType::union(empties);
    let when_non_empty = JType::union(non_empties);

    match true_kind {
        LengthKind::Empty => PredicateRefinement {
            when_true: when_empty,
            when_false: when_non_empty,
        },
        LengthKind::NonEmpty => PredicateRefinement {
            when_true: when_non_empty,
            when_false: when_empty,
        },
    }
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

#[derive(Clone, Copy)]
enum FlattenDepth {
    Full,
    Exact(usize),
    Unknown,
}

fn flatten_depth_from_type(input: &JType) -> FlattenDepth {
    match input {
        JType::Number(NumberType::Literal(value)) => value
            .parse::<usize>()
            .ok()
            .map(FlattenDepth::Exact)
            .unwrap_or(FlattenDepth::Unknown),
        _ => FlattenDepth::Unknown,
    }
}

fn flatten_item(input: JType, depth: FlattenDepth) -> JType {
    match depth {
        FlattenDepth::Full => flatten_item_full(input),
        FlattenDepth::Exact(depth) => flatten_item_exact(input, depth),
        FlattenDepth::Unknown => flatten_item_unknown_depth(input),
    }
}

fn flatten_item_full(input: JType) -> JType {
    match input {
        JType::Array(array) => flatten_item_full(*array.items),
        JType::Union(items) => JType::union(items.into_iter().map(flatten_item_full)),
        other => other,
    }
}

fn flatten_item_exact(input: JType, depth: usize) -> JType {
    if depth == 0 {
        return input;
    }

    match input {
        JType::Array(array) => flatten_item_exact(*array.items, depth - 1),
        JType::Union(items) => JType::union(
            items
                .into_iter()
                .map(|item| flatten_item_exact(item, depth)),
        ),
        other => other,
    }
}

fn flatten_item_unknown_depth(input: JType) -> JType {
    match input {
        JType::Array(array) => {
            let nested = (*array.items).clone();
            JType::union([
                JType::array(nested.clone()),
                flatten_item_unknown_depth(nested),
            ])
        }
        JType::Union(items) => JType::union(items.into_iter().map(flatten_item_unknown_depth)),
        other => other,
    }
}

fn flatten_union(input: JType) -> Vec<JType> {
    match input {
        JType::Union(items) => items,
        other => vec![other],
    }
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
    fn comparison_predicates_analyze_operands_for_diagnostics() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "teams": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": { "id": { "type": "string" } },
                        "required": ["id"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["teams"],
            "additionalProperties": false
        }));

        let direct_filter = "[.teams[] | select(.idNonExistant == \"1\")]";
        let direct = check(direct_filter, input.clone());
        let direct_diagnostic = direct
            .diagnostics
            .iter()
            .find(|diag| {
                diag.message
                    .contains("property \"idNonExistant\" is not present")
            })
            .expect("missing direct comparison diagnostic");
        let direct_start = direct_filter.find(".idNonExistant").unwrap();
        assert_eq!(
            direct_diagnostic.span,
            Some(SourceSpan::new(
                direct_start,
                direct_start + ".idNonExistant".len()
            ))
        );

        let piped_filter = "[.teams[] | select((.idNonExistant | tostring) == \"1\")]";
        let piped = check(piped_filter, input);
        let piped_diagnostic = piped
            .diagnostics
            .iter()
            .find(|diag| {
                diag.message
                    .contains("property \"idNonExistant\" is not present")
            })
            .expect("missing piped comparison diagnostic");
        let piped_start = piped_filter.find(".idNonExistant").unwrap();
        assert_eq!(
            piped_diagnostic.span,
            Some(SourceSpan::new(
                piped_start,
                piped_start + ".idNonExistant".len()
            ))
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
    fn flatten_range_and_numeric_builtins_are_analyzed() {
        let nested = json_schema_to_type(&json!({
            "type": "array",
            "items": {
                "anyOf": [
                    { "type": "number" },
                    {
                        "type": "array",
                        "items": {
                            "anyOf": [
                                { "type": "number" },
                                { "type": "array", "items": { "type": "number" } }
                            ]
                        }
                    }
                ]
            }
        }));

        let flattened = check("flatten", nested.clone());
        assert!(flattened.unsupported_features.is_empty());
        assert_eq!(flattened.output.to_compact_string(), "array<number>");

        let flattened_once = check("flatten(1)", nested);
        assert!(flattened_once.unsupported_features.is_empty());
        assert_eq!(
            flattened_once.output.to_compact_string(),
            "array<array<number> | number>"
        );

        let range = check("range(0; 3)", JType::Null);
        assert!(range.unsupported_features.is_empty());
        assert_eq!(
            range.output.to_compact_string(),
            "Stream<number, ZeroOrMore>"
        );

        let sin = check("sin", JType::number());
        assert!(sin.unsupported_features.is_empty());
        assert_eq!(sin.output.to_compact_string(), "number");

        let numeric = check(
            "{ cos: (1 | cos), ceil: (1.2 | ceil), pow: pow(2; 3), finite: (1 | isfinite), parts: (1.5 | modf), inf: infinite }",
            JType::Null,
        );
        assert!(numeric.unsupported_features.is_empty());
        let compact = numeric.output.to_compact_string();
        assert!(compact.contains("cos: number"));
        assert!(compact.contains("ceil: number"));
        assert!(compact.contains("pow: number"));
        assert!(compact.contains("finite: boolean"));
        assert!(compact.contains("parts: array<number>"));
        assert!(compact.contains("inf: number"));
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
    fn alt_lhs_suppresses_missing_property_warning() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "launches": {
                    "type": "array",
                    "items": { "type": "number" }
                }
            },
            "required": ["launches"],
            "additionalProperties": false
        }));

        let report = check(r#".query.status // "all""#, input);
        assert!(
            report.diagnostics.is_empty(),
            "expected no diagnostics, got: {:?}",
            report.diagnostics
        );
        assert_eq!(report.output.to_compact_string(), "\"all\"");
    }

    #[test]
    fn alt_lhs_chained_paths_suppress_all_missing_warnings() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "launches": {
                    "type": "array",
                    "items": { "type": "number" }
                }
            },
            "required": ["launches"],
            "additionalProperties": false
        }));

        let report = check(r#".query.status // .status // "all""#, input);
        assert!(
            report.diagnostics.is_empty(),
            "expected no diagnostics, got: {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn alt_lhs_nested_alts_keep_suppression() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "x": { "type": "string" }
            },
            "required": ["x"],
            "additionalProperties": false
        }));

        let report = check(r#"(.a.b // .c.d) // "fallback""#, input);
        assert!(
            report.diagnostics.is_empty(),
            "expected no diagnostics, got: {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn alt_rhs_still_warns_for_missing_property() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "x": { "type": "string" }
            },
            "required": ["x"],
            "additionalProperties": false
        }));

        let report = check(r#""default" // .missing"#, input);
        assert_eq!(report.diagnostics.len(), 1);
        assert!(
            report.diagnostics[0]
                .message
                .contains("property \"missing\" is not present")
        );
    }

    #[test]
    fn repeated_property_access_uses_correct_span() {
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
        let filter = ".params.id // .id";
        let report = check(filter, input);
        let diagnostic = report
            .diagnostics
            .iter()
            .find(|diag| diag.message.contains("property \"id\" is not present"))
            .expect("missing property diagnostic");
        let span = diagnostic.span.clone().expect("diagnostic span");

        assert_eq!(span, SourceSpan::new(14, 17));
        assert_eq!(&filter[span.start..span.end], ".id");
    }

    #[test]
    fn multiple_failing_accesses_get_distinct_spans() {
        let filter = ".foo + .foo";
        let report = check(filter, JType::closed_object(BTreeMap::new()));
        let spans: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|diag| diag.message.contains("property \"foo\" is not present"))
            .map(|diag| diag.span.clone())
            .collect();

        assert_eq!(
            spans,
            vec![Some(SourceSpan::new(0, 4)), Some(SourceSpan::new(7, 11))]
        );
    }

    #[test]
    fn unicode_source_prefixes_do_not_shift_repeated_access_spans() {
        let filter = "\"é\", .foo + .foo";
        let report = check(filter, JType::closed_object(BTreeMap::new()));
        let first = filter.find(".foo").unwrap();
        let second = filter.rfind(".foo").unwrap();
        let spans: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|diag| diag.message.contains("property \"foo\" is not present"))
            .map(|diag| diag.span.clone())
            .collect();

        assert_eq!(
            spans,
            vec![
                Some(SourceSpan::new(first, first + 4)),
                Some(SourceSpan::new(second, second + 4))
            ]
        );
    }

    #[test]
    fn predicate_analysis_keeps_repeated_access_spans_stable() {
        let filter = "if (.a | not) then .a else .b end";
        let report = check(filter, JType::closed_object(BTreeMap::new()));
        let spans: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|diag| diag.message.contains("property \"a\" is not present"))
            .map(|diag| diag.span.clone())
            .collect();

        assert_eq!(
            spans,
            vec![Some(SourceSpan::new(4, 6)), Some(SourceSpan::new(19, 21))]
        );
    }

    #[test]
    fn branches_with_repeated_keys_report_each_branch_location() {
        let mut props = BTreeMap::new();
        props.insert("flag".to_string(), JType::property(JType::bool(), true));
        let filter = "if .flag then .a else .a end";
        let report = check(filter, JType::closed_object(props));
        let spans: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|diag| diag.message.contains("property \"a\" is not present"))
            .map(|diag| diag.span.clone())
            .collect();

        assert_eq!(
            spans,
            vec![Some(SourceSpan::new(14, 16)), Some(SourceSpan::new(22, 24))]
        );
    }

    #[test]
    fn outside_alt_still_warns_for_missing_property() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "x": { "type": "string" }
            },
            "required": ["x"],
            "additionalProperties": false
        }));

        let report = check(r#"(.a // "x") + .b"#, input);
        assert_eq!(report.diagnostics.len(), 1);
        assert!(
            report.diagnostics[0]
                .message
                .contains("property \"b\" is not present")
        );
    }

    fn launch_error_union_schema() -> serde_json::Value {
        json!({
            "anyOf": [
                {
                    "type": "object",
                    "properties": {
                        "errors": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "type": { "type": "string" },
                                    "message": { "type": "string" }
                                },
                                "required": ["type", "message"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["errors"],
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "properties": {
                        "launch": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "number" },
                                "name": { "type": "string" }
                            },
                            "required": ["id", "name"],
                            "additionalProperties": false
                        }
                    },
                    "required": ["launch"],
                    "additionalProperties": false
                }
            ]
        })
    }

    #[test]
    fn if_length_gt_zero_narrows_union_into_present_field_member() {
        let input = json_schema_to_type(&launch_error_union_schema());

        let report = check(
            "if ((.errors // []) | length) > 0 then .errors[0] else .launch end",
            input,
        );
        assert!(
            report.diagnostics.is_empty(),
            "expected no diagnostics, got: {:?}",
            report.diagnostics
        );
        let compact = report.output.to_compact_string();
        // Then branch refines to the {errors} member, so .errors[0] is Err | null
        // (array indexing); else branch refines to the {launch} member, so .launch
        // is the launch object. Together: Err | null | launch_object.
        assert!(
            compact.contains("type: string"),
            "expected error item shape in {compact}",
        );
        assert!(
            compact.contains("id: number"),
            "expected launch shape in {compact}",
        );
    }

    #[test]
    fn if_length_eq_zero_narrows_to_absent_or_empty() {
        let input = json_schema_to_type(&launch_error_union_schema());

        let report = check(
            "if ((.errors // []) | length) == 0 then .launch else .errors end",
            input,
        );
        assert!(
            report.diagnostics.is_empty(),
            "expected no diagnostics, got: {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn length_not_eq_zero_behaves_like_gt_zero() {
        let input = json_schema_to_type(&launch_error_union_schema());

        let report = check(
            "if ((.errors // []) | length) != 0 then .errors else .launch end",
            input,
        );
        assert!(
            report.diagnostics.is_empty(),
            "expected no diagnostics, got: {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn length_predicate_without_alt_default_still_narrows() {
        let input = json_schema_to_type(&launch_error_union_schema());

        // No `// []` default. The pattern should still be recognized for
        // narrowing purposes.
        let report = check(
            "if (.errors | length) > 0 then .errors else .launch end",
            input,
        );
        assert!(
            report.diagnostics.is_empty(),
            "expected no diagnostics, got: {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn length_predicate_only_narrows_when_field_disambiguates() {
        // Single union member that has BOTH fields. Predicate cannot disambiguate
        // (there are no other members where errors is missing). No narrowing.
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "errors": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "launch": {
                    "type": "object",
                    "properties": { "id": { "type": "number" } },
                    "required": ["id"],
                    "additionalProperties": false
                }
            },
            "required": ["errors", "launch"],
            "additionalProperties": false
        }));

        let report = check(
            "if (.errors | length) > 0 then .launch else .launch end",
            input,
        );
        // .launch is present on the single member, so this should not warn.
        assert!(
            report.diagnostics.is_empty(),
            "expected no diagnostics, got: {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn length_predicate_n_other_than_zero_or_one_does_not_narrow() {
        let input = json_schema_to_type(&launch_error_union_schema());

        // `length > 5` is not a recognized partition; falls through to generic.
        let report = check(
            "if (.errors | length) > 5 then .errors else .launch end",
            input,
        );
        // Both branches see the full union, so .launch on the {errors} member
        // emits a missing-property warning.
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.message.contains("property \"launch\" is not present")),
            "expected missing-launch warning, got: {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn alt_default_string_or_object_recognized() {
        // String default.
        let input = json_schema_to_type(&json!({
            "anyOf": [
                {
                    "type": "object",
                    "properties": { "text": { "type": "string" } },
                    "required": ["text"],
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "properties": { "other": { "type": "number" } },
                    "required": ["other"],
                    "additionalProperties": false
                }
            ]
        }));
        let report = check(
            r#"if ((.text // "") | length) > 0 then .text else .other end"#,
            input,
        );
        assert!(
            report.diagnostics.is_empty(),
            "expected no diagnostics (string default), got: {:?}",
            report.diagnostics
        );

        // Object default.
        let input = json_schema_to_type(&json!({
            "anyOf": [
                {
                    "type": "object",
                    "properties": {
                        "bag": {
                            "type": "object",
                            "properties": { "k": { "type": "string" } },
                            "required": ["k"],
                            "additionalProperties": false
                        }
                    },
                    "required": ["bag"],
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "properties": { "other": { "type": "number" } },
                    "required": ["other"],
                    "additionalProperties": false
                }
            ]
        }));
        let report = check(
            "if ((.bag // {}) | length) > 0 then .bag else .other end",
            input,
        );
        assert!(
            report.diagnostics.is_empty(),
            "expected no diagnostics (object default), got: {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn webpipe_repro_launch_detail_else_branch() {
        let input = json_schema_to_type(&json!({
            "anyOf": [
                {
                    "type": "object",
                    "properties": {
                        "errors": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "type": { "type": "string" },
                                    "message": { "type": "string" }
                                },
                                "required": ["type", "message"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["errors"],
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "properties": {
                        "launch": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "number" },
                                "slug": { "type": "string" },
                                "name": { "type": "string" }
                            },
                            "required": ["id", "slug", "name"],
                            "additionalProperties": false
                        }
                    },
                    "required": ["launch"],
                    "additionalProperties": false
                }
            ]
        }));

        let report = check(
            "if ((.errors // []) | length) > 0 then { errors: .errors } else .launch as $launch | { launch: $launch } end",
            input,
        );
        assert!(
            report.diagnostics.is_empty(),
            "expected no diagnostics, got: {:?}",
            report.diagnostics
        );
        let compact = report.output.to_compact_string();
        assert!(
            !compact.contains("null"),
            "expected no null in output, got: {compact}",
        );
    }

    #[test]
    fn alt_lhs_still_reports_unrelated_type_errors() {
        // `.x` is a string; `.x.k` is "field may be applied to non-object".
        // That is not a missing-property warning, so the `//` LHS gating
        // must not suppress it.
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "x": { "type": "string" }
            },
            "required": ["x"],
            "additionalProperties": false
        }));

        let report = check(r#".x.k // "ok""#, input);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.message.contains("may be applied to non-object")),
            "expected non-object access warning, got: {:?}",
            report.diagnostics
        );
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
    fn def_with_no_args_inlines_body() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": { "count": { "type": "number" } },
            "required": ["count"],
            "additionalProperties": false
        }));
        let report = check("def increment: . + 1; .count | increment", input);
        assert!(report.unsupported_features.is_empty());
        assert_eq!(report.output.to_compact_string(), "number");
    }

    #[test]
    fn def_with_filter_arg_substitutes() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": { "x": { "type": "number" } },
            "required": ["x"],
            "additionalProperties": false
        }));
        let report = check("def f(g): g + 1; .x | f(. * 2)", input);
        assert!(report.unsupported_features.is_empty());
        assert_eq!(report.output.to_compact_string(), "number");
    }

    #[test]
    fn def_with_value_arg_binds_var() {
        let report = check("def add($n): . + $n; 10 | add(5)", JType::Unknown);
        assert!(report.unsupported_features.is_empty());
        assert_eq!(report.output.to_compact_string(), "number");
    }

    #[test]
    fn recursive_def_widens_after_depth_cap() {
        let report = check(
            "def loop: if . > 0 then (. - 1 | loop) else . end; 5 | loop",
            JType::Unknown,
        );
        // Should not crash; widens via fixed-point convergence
        let _ = report.output.to_compact_string();
    }

    #[test]
    fn slice_assignment_widens_array_item_type() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            },
            "required": ["items"],
            "additionalProperties": false
        }));
        let report = check(".items[2:4] = [{id: 0}]", input);
        assert!(report.unsupported_features.is_empty());
        let compact = report.output.to_compact_string();
        assert!(compact.contains("items: array<"));
        assert!(compact.contains("string"));
        assert!(compact.contains("object{id: 0}"));
    }

    #[test]
    fn nested_dynamic_assignment_chains() {
        // Use literal-string dynamic keys so both sides resolve concretely
        let input = JType::Unknown;
        let report = check(r#".["outer"]["inner"] = 1"#, input);
        assert!(report.unsupported_features.is_empty());
        let compact = report.output.to_compact_string();
        assert!(compact.contains("outer"));
        assert!(compact.contains("inner"));
    }

    #[test]
    fn recursive_descent_returns_descendants() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "a": { "type": "number" },
                "b": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            },
            "required": ["a", "b"],
            "additionalProperties": false
        }));
        let report = check("..", input);
        assert!(report.unsupported_features.is_empty());
        let compact = report.output.to_compact_string();
        assert!(compact.contains("ZeroOrMore"));
        assert!(compact.contains("number"));
        assert!(compact.contains("string"));
        assert!(compact.contains("array<string>"));
    }

    #[test]
    fn group_by_returns_nested_array() {
        let input = JType::array(JType::number());
        let report = check("group_by(.)", input);
        assert!(report.unsupported_features.is_empty());
        assert_eq!(report.output.to_compact_string(), "array<array<number>>");
    }

    #[test]
    fn sort_returns_same_array_type() {
        let input = JType::array(JType::string());
        let report = check("sort", input);
        assert!(report.unsupported_features.is_empty());
        assert_eq!(report.output.to_compact_string(), "array<string>");
    }

    #[test]
    fn min_returns_item_or_null() {
        let input = JType::array(JType::number());
        let report = check("min", input);
        assert!(report.unsupported_features.is_empty());
        assert_eq!(report.output.to_compact_string(), "null | number");
    }

    #[test]
    fn to_entries_reshapes_object() {
        let input = json_schema_to_type(&json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "number" }
            },
            "required": ["name", "age"],
            "additionalProperties": false
        }));
        let report = check("to_entries", input);
        assert!(report.unsupported_features.is_empty());
        let compact = report.output.to_compact_string();
        assert!(compact.starts_with("array<object{"));
        assert!(compact.contains("key: string"));
        assert!(compact.contains("value: number | string"));
    }

    #[test]
    fn from_entries_reshapes_to_object() {
        let input = json_schema_to_type(&json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "key": { "type": "string" },
                    "value": { "type": "number" }
                },
                "required": ["key", "value"],
                "additionalProperties": false
            }
        }));
        let report = check("from_entries", input);
        assert!(report.unsupported_features.is_empty());
        assert_eq!(report.output.to_compact_string(), "object{...: number}");
    }

    #[test]
    fn match_returns_match_object() {
        let report = check(r#"match("foo")"#, JType::string());
        assert!(report.unsupported_features.is_empty());
        let compact = report.output.to_compact_string();
        assert!(compact.contains("offset: number"));
        assert!(compact.contains("length: number"));
        assert!(compact.contains("string: string"));
        assert!(compact.contains("captures: array<object"));
    }

    #[test]
    fn startswith_returns_bool() {
        let report = check(r#"startswith("foo")"#, JType::string());
        assert!(report.unsupported_features.is_empty());
        assert_eq!(report.output.to_compact_string(), "boolean");
    }

    #[test]
    fn index_returns_number_or_null() {
        let report = check(r#"index("foo")"#, JType::string());
        assert!(report.unsupported_features.is_empty());
        assert_eq!(report.output.to_compact_string(), "null | number");
    }

    #[test]
    fn first_zero_arg_returns_array_item_or_null() {
        let report = check("first", JType::array(JType::string()));
        assert!(report.unsupported_features.is_empty());
        assert_eq!(report.output.to_compact_string(), "null | string");
    }

    #[test]
    fn split_returns_string_array() {
        let report = check(r#"split(",")"#, JType::string());
        assert!(report.unsupported_features.is_empty());
        assert_eq!(report.output.to_compact_string(), "array<string>");
    }

    #[test]
    fn tojson_returns_string() {
        let input = JType::array(JType::number());
        let report = check("tojson", input);
        assert!(report.unsupported_features.is_empty());
        assert_eq!(report.output.to_compact_string(), "string");
    }

    #[test]
    fn error_returns_zero() {
        let report = check(r#"error("nope")"#, JType::Unknown);
        assert!(report.unsupported_features.is_empty());
        assert!(matches!(report.output.card, Cardinality::Zero));
    }

    #[test]
    fn unsupported_builtin_reports_warning() {
        let report = check("not_a_real_builtin", JType::array(JType::Unknown));
        assert_eq!(report.output.to_compact_string(), "unknown");
        assert_eq!(report.unsupported_features.len(), 1);
        assert!(
            report.diagnostics[0]
                .message
                .contains("unsupported builtin or call `not_a_real_builtin`")
        );
    }
}
