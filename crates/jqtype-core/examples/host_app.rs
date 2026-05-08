//! End-to-end embedding example showing how a host application can use
//! `jqtype-core` as a library — without depending on the `jqtype` CLI.
//!
//! Run with:
//!
//! ```sh
//! cargo run --example host_app -p jqtype-core
//! ```
//!
//! The example:
//!  1. Builds a [`JType`] directly to mirror a host's internal type model.
//!  2. Loads a JSON Schema for a different filter to show the
//!     [`InputShape::JsonSchema`] path.
//!  3. Verifies a concrete value against the inferred output type using
//!     [`value_fits_type`], which is the same predicate the compatibility
//!     harness uses.
//!  4. Serializes the [`AnalyzeReport`] to JSON for transport across a
//!     process boundary.

use std::collections::BTreeMap;

use jqtype_core::{AnalyzeOptions, InputShape, JType, JqTypeChecker, Property, value_fits_type};
use serde_json::json;

fn main() {
    let checker = JqTypeChecker::new();

    let mut params = BTreeMap::new();
    params.insert("world".to_string(), JType::property(JType::string(), true));

    let mut route = BTreeMap::new();
    route.insert(
        "params".to_string(),
        Property {
            ty: JType::closed_object(params),
            required: true,
        },
    );
    let route_input = JType::closed_object(route);

    let direct = checker.analyze_filter(
        "{ world: .params.world }",
        InputShape::from_type(route_input),
        AnalyzeOptions::default(),
    );
    println!(
        "direct JType -> {}",
        direct.output_type().to_compact_string()
    );

    let schema = json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "number" },
                        "name": { "type": "string" }
                    },
                    "required": ["id", "name"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["items"],
        "additionalProperties": false
    });

    let projection = checker.analyze_filter(
        ".items[] | { id, name }",
        InputShape::from_json_schema(schema),
        AnalyzeOptions::default(),
    );

    println!(
        "json schema -> {}",
        projection.output_type().to_compact_string()
    );

    let actual = json!({ "id": 7, "name": "Ada" });
    assert!(
        value_fits_type(&actual, &projection.output_type().item),
        "embedding contract: concrete value should fit inferred output"
    );

    let serialized = serde_json::to_string_pretty(&projection)
        .expect("AnalyzeReport must serialize for IPC/diagnostics");
    println!("---\nserialized report:\n{serialized}");
}
