use std::collections::BTreeMap;

use jqtype_core::{AnalyzeOptions, InputShape, JType, JqTypeChecker};

fn main() {
    let mut params = BTreeMap::new();
    params.insert("world".to_string(), JType::property(JType::string(), true));

    let mut route = BTreeMap::new();
    route.insert(
        "params".to_string(),
        JType::property(JType::closed_object(params), true),
    );

    let report = JqTypeChecker::new().analyze_filter(
        "{ world: .params.world }",
        InputShape::Type(JType::closed_object(route)),
        AnalyzeOptions::default(),
    );

    println!("{}", report.output_type().to_compact_string());
}
