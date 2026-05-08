//! Static JSON shape analysis for jq filters.
//!
//! `jqtype-core` parses a jq filter, threads an inferred [`JType`] through
//! every operator and builtin it understands, and returns a [`StreamType`]
//! that captures both the possible item shape and the cardinality
//! (`Zero`/`One`/`ZeroOrOne`/`OneOrMore`/`ZeroOrMore`) of the filter's
//! output stream.
//!
//! The library is the primary product surface. The `jqtype` CLI is just a
//! thin wrapper. Embedding applications should depend on this crate
//! directly and call [`JqTypeChecker::analyze_filter`].
//!
//! # Quick start
//!
//! ```
//! use jqtype_core::{AnalyzeOptions, InputShape, JqTypeChecker};
//! use serde_json::json;
//!
//! let schema = json!({
//!     "type": "object",
//!     "properties": {
//!         "items": {
//!             "type": "array",
//!             "items": {
//!                 "type": "object",
//!                 "properties": {
//!                     "id": { "type": "number" },
//!                     "name": { "type": "string" }
//!                 },
//!                 "required": ["id", "name"],
//!                 "additionalProperties": false
//!             }
//!         }
//!     },
//!     "required": ["items"],
//!     "additionalProperties": false
//! });
//!
//! let report = JqTypeChecker::new().analyze_filter(
//!     ".items[] | { id, name }",
//!     InputShape::from_json_schema(schema),
//!     AnalyzeOptions::default(),
//! );
//!
//! assert_eq!(
//!     report.output_type().to_compact_string(),
//!     "Stream<object{id: number, name: string}, ZeroOrMore>"
//! );
//! ```
//!
//! # Embedding
//!
//! Host applications that already have an inferred shape can pass it in
//! directly via [`InputShape::Type`]; callers with JSON interchange data
//! can use [`InputShape::JsonSchema`] or [`InputShape::Sample`]. Every
//! report type implements `serde::Serialize`/`Deserialize`, so reports can
//! flow over IPC or be rendered as JSON.

mod analyze;
mod diagnostic;
mod schema;
mod stream;
mod types;

pub use analyze::{
    AnalysisMode, AnalyzeOptions, AnalyzeReport, InputShape, JqTypeChecker, OutputFormat,
    UnsupportedFeature,
};
pub use diagnostic::{Diagnostic, Severity, SourceSpan};
pub use schema::{json_schema_to_type, sample_to_type, type_to_json_schema, value_fits_type};
pub use stream::{Cardinality, StreamType};
pub use types::{ArrayType, BoolType, JType, NumberType, ObjectType, Property, StringType};
