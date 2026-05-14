//! Compatibility harness: run a real jq on curated samples and verify that
//! every concrete output value fits the type inferred by `jqtype-core`.
//!
//! This is the soundness backstop the implementation plan calls for in
//! Milestone 7. If jq disagrees with the static analyzer, the inferred type
//! excludes a possible runtime output and we have a soundness bug.
//!
//! The harness skips itself when no `jq` binary is installed.

use std::io::Write;
use std::process::{Command, Stdio};

use jqtype_core::{
    AnalyzeOptions, Cardinality, InputShape, JqTypeChecker, StreamType, value_fits_type,
};
use serde_json::{Value, json};

struct Case {
    name: &'static str,
    filter: &'static str,
    input_schema: Value,
    inputs: Vec<Value>,
}

fn cases() -> Vec<Case> {
    let users_schema = json!({
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

    let users_sample = json!({
        "items": [
            { "id": 1, "name": "Ada" },
            { "id": 2, "name": "Grace" }
        ]
    });

    let empty_users = json!({ "items": [] });

    let discriminated_schema = json!({
        "type": "array",
        "items": {
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
        }
    });

    let discriminated_sample = json!([
        { "type": "user", "name": "Ada" },
        { "type": "org", "org_name": "Anthropic" },
        { "type": "user", "name": "Grace" }
    ]);

    let active_schema = json!({
        "type": "object",
        "properties": {
            "active": { "type": "boolean" },
            "name": { "type": "string" }
        },
        "required": ["active", "name"],
        "additionalProperties": false
    });

    let nullable_foo_schema = json!({
        "type": "object",
        "properties": {
            "foo": { "type": ["string", "null"] }
        },
        "required": ["foo"],
        "additionalProperties": false
    });

    let route_params_schema = json!({
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
    });

    let rows_by_team_schema = json!({
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
                            "required": ["team_id", "name"],
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
    });

    let hourly_weather_schema = json!({
        "type": "object",
        "properties": {
            "data": {
                "type": "object",
                "properties": {
                    "response": {
                        "type": "object",
                        "properties": {
                            "hourly": {
                                "type": "object",
                                "properties": {
                                    "time": { "type": "array", "items": { "type": "string" } },
                                    "temperature_2m": { "type": "array", "items": { "type": "number" } }
                                },
                                "required": ["time", "temperature_2m"],
                                "additionalProperties": false
                            }
                        },
                        "required": ["hourly"],
                        "additionalProperties": false
                    }
                },
                "required": ["response"],
                "additionalProperties": false
            }
        },
        "required": ["data"],
        "additionalProperties": false
    });

    let nested_numbers_schema = json!({
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
    });

    let nested_numbers_sample = json!([1, [2, [3]], 4]);

    vec![
        Case {
            name: "identity",
            filter: ".",
            input_schema: users_schema.clone(),
            inputs: vec![users_sample.clone(), empty_users.clone()],
        },
        Case {
            name: "field projection",
            filter: ".items",
            input_schema: users_schema.clone(),
            inputs: vec![users_sample.clone(), empty_users.clone()],
        },
        Case {
            name: "iterate items",
            filter: ".items[]",
            input_schema: users_schema.clone(),
            inputs: vec![users_sample.clone(), empty_users.clone()],
        },
        Case {
            name: "collect projection",
            filter: "[.items[].name]",
            input_schema: users_schema.clone(),
            inputs: vec![users_sample.clone(), empty_users.clone()],
        },
        Case {
            name: "object construction",
            filter: ".items[] | {id, name}",
            input_schema: users_schema.clone(),
            inputs: vec![users_sample.clone(), empty_users.clone()],
        },
        Case {
            name: "if then else over booleans",
            filter: "if .active then .name else null end",
            input_schema: active_schema.clone(),
            inputs: vec![
                json!({"active": true, "name": "Ada"}),
                json!({"active": false, "name": "Grace"}),
            ],
        },
        Case {
            name: "select on discriminated union",
            filter: ".[] | select(.type == \"user\") | .name",
            input_schema: discriminated_schema.clone(),
            inputs: vec![discriminated_sample.clone(), json!([])],
        },
        Case {
            name: "comma combines streams",
            filter: ".items[0].id, .items[0].name",
            input_schema: users_schema.clone(),
            inputs: vec![users_sample.clone()],
        },
        Case {
            name: "map projection",
            filter: ".items | map(.id)",
            input_schema: users_schema.clone(),
            inputs: vec![users_sample.clone(), empty_users.clone()],
        },
        Case {
            name: "type builtin",
            filter: ".items[] | type",
            input_schema: users_schema.clone(),
            inputs: vec![users_sample.clone()],
        },
        Case {
            name: "length builtin",
            filter: ".items | length",
            input_schema: users_schema.clone(),
            inputs: vec![users_sample.clone(), empty_users.clone()],
        },
        Case {
            name: "keys on object",
            filter: ".items[0] | keys",
            input_schema: users_schema.clone(),
            inputs: vec![users_sample.clone()],
        },
        Case {
            name: "empty produces no outputs",
            filter: "empty",
            input_schema: users_schema.clone(),
            inputs: vec![users_sample.clone()],
        },
        Case {
            name: "select non-null then field",
            filter: "select(.foo != null) | .foo",
            input_schema: nullable_foo_schema.clone(),
            inputs: vec![json!({"foo": "x"}), json!({"foo": null})],
        },
        Case {
            name: "string filter narrows union",
            filter: ".foo | strings",
            input_schema: nullable_foo_schema,
            inputs: vec![json!({"foo": "x"}), json!({"foo": null})],
        },
        Case {
            // jq's `+` is overloaded across numbers/strings/arrays/objects;
            // the analyzer widens to Unknown rather than guessing.
            name: "math op stays sound on string concat",
            filter: ".items[0].name + \"!\"",
            input_schema: users_schema.clone(),
            inputs: vec![users_sample.clone()],
        },
        Case {
            name: "conversion and string concat",
            filter: "{ id: (.params.id | tonumber), label: (\"Team \" + (.params.id | tostring)) }",
            input_schema: route_params_schema.clone(),
            inputs: vec![json!({"params": {"id": "42"}})],
        },
        Case {
            name: "alternative fallback with repeated key",
            filter: ".params.id // .id",
            input_schema: route_params_schema.clone(),
            inputs: vec![json!({"params": {"id": "42"}})],
        },
        Case {
            name: "identity-root assignment",
            filter: ".graphqlParams = { id: 1 }",
            input_schema: route_params_schema,
            inputs: vec![json!({"params": {"id": "42"}})],
        },
        Case {
            name: "as binding with transpose",
            filter: ".data.response.hourly as $h | [$h.time, $h.temperature_2m] | transpose | map({time: .[0], temp: .[1]})",
            input_schema: hourly_weather_schema,
            inputs: vec![json!({
                "data": {
                    "response": {
                        "hourly": {
                            "time": ["2026-01-09T00:00", "2026-01-09T01:00"],
                            "temperature_2m": [-4.2, -4.4]
                        }
                    }
                }
            })],
        },
        Case {
            name: "reduce dynamic grouping update",
            filter: "reduce .data.rows[] as $row ({}; .[$row.team_id | tostring] += [$row])",
            input_schema: rows_by_team_schema,
            inputs: vec![
                json!({
                    "data": {
                        "rows": [
                            { "team_id": 1, "name": "Platform" },
                            { "team_id": 1, "name": "Growth" },
                            { "team_id": 2, "name": "Security" }
                        ]
                    }
                }),
                json!({"data": {"rows": []}}),
            ],
        },
        Case {
            name: "slice interpolation and fallback",
            filter: "{ preview: (.body | .[0:5] + \"...\"), source: (.source // \"fallback\") }",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "body": { "type": "string" },
                    "source": { "type": ["string", "null"] }
                },
                "required": ["body", "source"],
                "additionalProperties": false
            }),
            inputs: vec![
                json!({"body": "abcdefgh", "source": "primary"}),
                json!({"body": "abc", "source": null}),
            ],
        },
        Case {
            name: "first array item",
            filter: ".items | first",
            input_schema: users_schema.clone(),
            inputs: vec![users_sample.clone(), empty_users],
        },
        Case {
            name: "index returns position or null",
            filter: ".items | first | .name | index(\"a\")",
            input_schema: users_schema.clone(),
            inputs: vec![users_sample.clone(), json!({"items": []})],
        },
        Case {
            name: "add and join builtins",
            filter: "{ total: ([10, 20, 30] | add), joined: (.items | map(.id | tostring) | join(\",\")) }",
            input_schema: users_schema,
            inputs: vec![users_sample],
        },
        Case {
            name: "flatten builtin",
            filter: "flatten",
            input_schema: nested_numbers_schema.clone(),
            inputs: vec![nested_numbers_sample.clone(), json!([])],
        },
        Case {
            name: "flatten depth builtin",
            filter: "flatten(1)",
            input_schema: nested_numbers_schema,
            inputs: vec![nested_numbers_sample, json!([])],
        },
        Case {
            name: "numeric math builtins",
            filter: "{ cos: (1 | cos), ceil: (1.2 | ceil), pow: pow(2; 3), finite: (1 | isfinite), parts: (1.5 | modf), rangeSin: [range(0; 3) | sin] }",
            input_schema: json!({}),
            inputs: vec![json!(null), json!({"ignored": true})],
        },
    ]
}

fn jq_available() -> bool {
    Command::new("jq")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn run_jq(filter: &str, input: &Value) -> Result<Vec<Value>, String> {
    let mut child = Command::new("jq")
        .arg("-c")
        .arg(filter)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("failed to spawn jq: {err}"))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "missing jq stdin".to_string())?;
        stdin
            .write_all(serde_json::to_string(input).unwrap().as_bytes())
            .map_err(|err| format!("failed to write jq stdin: {err}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|err| format!("failed to wait on jq: {err}"))?;

    if !output.status.success() {
        return Err(format!(
            "jq exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|err| format!("jq stdout was not utf-8: {err}"))?;

    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .map_err(|err| format!("could not parse jq output line `{line}`: {err}"))
        })
        .collect()
}

#[test]
fn jq_outputs_fit_inferred_types() {
    if !jq_available() {
        eprintln!("skipping compatibility harness: `jq` not found in PATH");
        return;
    }

    let checker = JqTypeChecker::new();
    let mut failures: Vec<String> = Vec::new();

    for case in cases() {
        let report = checker.analyze_filter(
            case.filter,
            InputShape::from_json_schema(case.input_schema.clone()),
            AnalyzeOptions::default(),
        );

        if report.has_errors() {
            failures.push(format!(
                "case `{}`: analysis produced errors: {:?}",
                case.name, report.diagnostics
            ));
            continue;
        }

        let stream: &StreamType = report.output_type();

        for (index, input) in case.inputs.iter().enumerate() {
            let outputs = match run_jq(case.filter, input) {
                Ok(outputs) => outputs,
                Err(err) => {
                    failures.push(format!(
                        "case `{}` input #{index}: jq invocation failed: {err}",
                        case.name
                    ));
                    continue;
                }
            };

            if !stream.card.fits_count(outputs.len()) {
                failures.push(format!(
                    "case `{}` input #{index}: produced {} outputs, which does not fit cardinality {:?} (inferred type {})",
                    case.name,
                    outputs.len(),
                    stream.card,
                    stream.to_compact_string()
                ));
                continue;
            }

            for (output_index, value) in outputs.iter().enumerate() {
                if !value_fits_type(value, &stream.item) {
                    failures.push(format!(
                        "case `{}` input #{index} output #{output_index}: value {value} does not fit inferred item type {}",
                        case.name,
                        stream.item.to_compact_string()
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} compatibility failure(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn cardinality_fits_count_matrix() {
    use Cardinality::*;
    assert!(Zero.fits_count(0));
    assert!(!Zero.fits_count(1));
    assert!(One.fits_count(1));
    assert!(!One.fits_count(0));
    assert!(!One.fits_count(2));
    assert!(ZeroOrOne.fits_count(0));
    assert!(ZeroOrOne.fits_count(1));
    assert!(!ZeroOrOne.fits_count(2));
    assert!(!OneOrMore.fits_count(0));
    assert!(OneOrMore.fits_count(1));
    assert!(OneOrMore.fits_count(7));
    assert!(ZeroOrMore.fits_count(0));
    assert!(ZeroOrMore.fits_count(1));
    assert!(ZeroOrMore.fits_count(99));
}
