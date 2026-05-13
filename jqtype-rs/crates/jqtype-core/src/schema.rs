use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Number, Value, json};

use crate::types::{ArrayType, BoolType, JType, NumberType, ObjectType, Property, StringType};

pub fn sample_to_type(value: &Value) -> JType {
    match value {
        Value::Null => JType::Null,
        Value::Bool(value) => JType::bool_lit(*value),
        Value::Number(value) => JType::number_lit(value.to_string()),
        Value::String(value) => JType::string_lit(value.clone()),
        Value::Array(values) => {
            let item = JType::union(values.iter().map(sample_to_type));
            JType::array(item)
        }
        Value::Object(values) => {
            let properties = values
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        Property {
                            ty: sample_to_type(value),
                            required: true,
                        },
                    )
                })
                .collect();
            JType::closed_object(properties)
        }
    }
}

pub fn json_schema_to_type(schema: &Value) -> JType {
    let Some(schema) = schema.as_object() else {
        return JType::Unknown;
    };

    if let Some(enum_values) = schema.get("enum").and_then(Value::as_array) {
        return JType::union(enum_values.iter().map(sample_to_type));
    }

    if let Some(any_of) = schema.get("anyOf").and_then(Value::as_array) {
        return JType::union(any_of.iter().map(json_schema_to_type));
    }

    if let Some(one_of) = schema.get("oneOf").and_then(Value::as_array) {
        return JType::union(one_of.iter().map(json_schema_to_type));
    }

    if let Some(all_of) = schema.get("allOf").and_then(Value::as_array) {
        return merge_all_of(all_of);
    }

    match schema.get("type") {
        Some(Value::String(kind)) => schema_type_to_type(kind, schema),
        Some(Value::Array(kinds)) => JType::union(
            kinds
                .iter()
                .filter_map(Value::as_str)
                .map(|kind| schema_type_to_type(kind, schema)),
        ),
        _ if schema.contains_key("properties") => object_schema_to_type(schema),
        _ if schema.contains_key("items") => array_schema_to_type(schema),
        _ => JType::Unknown,
    }
}

pub fn type_to_json_schema(ty: &JType) -> Value {
    match ty {
        JType::Never => Value::Bool(false),
        JType::Unknown => json!({}),
        JType::Null => json!({ "type": "null" }),
        JType::Bool(BoolType::Any) => json!({ "type": "boolean" }),
        JType::Bool(BoolType::Literal(value)) => json!({ "const": value }),
        JType::Number(NumberType::Any) => json!({ "type": "number" }),
        JType::Number(NumberType::Literal(value)) => {
            let parsed = value.parse::<f64>().ok().map(Value::from);
            json!({ "const": parsed.unwrap_or_else(|| Value::String(value.clone())) })
        }
        JType::String(StringType::Any) => json!({ "type": "string" }),
        JType::String(StringType::Literal(value)) => json!({ "const": value }),
        JType::Array(array) => {
            json!({
                "type": "array",
                "items": type_to_json_schema(&array.items),
            })
        }
        JType::Object(object) => object_to_json_schema(object),
        JType::Union(items) => json!({
            "anyOf": items.iter().map(type_to_json_schema).collect::<Vec<_>>()
        }),
    }
}

fn schema_type_to_type(kind: &str, schema: &Map<String, Value>) -> JType {
    match kind {
        "null" => JType::Null,
        "boolean" => JType::bool(),
        "number" | "integer" => JType::number(),
        "string" => JType::string(),
        "array" => array_schema_to_type(schema),
        "object" => object_schema_to_type(schema),
        _ => JType::Unknown,
    }
}

fn array_schema_to_type(schema: &Map<String, Value>) -> JType {
    let item = match schema.get("items") {
        Some(Value::Array(items)) => JType::union(items.iter().map(json_schema_to_type)),
        Some(item_schema) => json_schema_to_type(item_schema),
        None => JType::Unknown,
    };
    JType::array(item)
}

fn object_schema_to_type(schema: &Map<String, Value>) -> JType {
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();

    let mut properties = BTreeMap::new();
    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        for (key, prop_schema) in props {
            properties.insert(
                key.clone(),
                Property {
                    ty: json_schema_to_type(prop_schema),
                    required: required.contains(key),
                },
            );
        }
    }

    let additional = match schema.get("additionalProperties") {
        Some(Value::Bool(false)) => None,
        Some(Value::Bool(true)) | None => Some(JType::Unknown),
        Some(value) => Some(json_schema_to_type(value)),
    };

    JType::object(properties, additional)
}

fn merge_all_of(items: &[Value]) -> JType {
    let mut properties = BTreeMap::new();
    let mut additional = Some(JType::Unknown);

    for item in items {
        match json_schema_to_type(item) {
            JType::Object(object) => {
                for (key, prop) in object.properties {
                    properties
                        .entry(key)
                        .and_modify(|existing: &mut Property| {
                            existing.ty = JType::union([existing.ty.clone(), prop.ty.clone()]);
                            existing.required |= prop.required;
                        })
                        .or_insert(prop);
                }
                if object.additional.is_none() {
                    additional = None;
                }
            }
            other => return other,
        }
    }

    JType::object(properties, additional)
}

/// Returns `true` when `value` is a possible runtime instance of `ty`.
///
/// Used by the compatibility harness and by host applications that want to
/// validate concrete data against an inferred [`JType`]. The check is
/// conservative: [`JType::Unknown`] accepts anything, [`JType::Never`]
/// accepts nothing, unions accept any matching member, and open object
/// types accept extra properties.
pub fn value_fits_type(value: &Value, ty: &JType) -> bool {
    match ty {
        JType::Never => false,
        JType::Unknown => true,
        JType::Null => value.is_null(),
        JType::Bool(BoolType::Any) => value.is_boolean(),
        JType::Bool(BoolType::Literal(expected)) => {
            value.as_bool().is_some_and(|actual| actual == *expected)
        }
        JType::Number(NumberType::Any) => value.is_number(),
        JType::Number(NumberType::Literal(expected)) => match value {
            Value::Number(actual) => number_eq(actual, expected),
            _ => false,
        },
        JType::String(StringType::Any) => value.is_string(),
        JType::String(StringType::Literal(expected)) => {
            value.as_str().is_some_and(|actual| actual == expected)
        }
        JType::Array(ArrayType { items }) => match value {
            Value::Array(values) => values.iter().all(|item| value_fits_type(item, items)),
            _ => false,
        },
        JType::Object(object) => value
            .as_object()
            .is_some_and(|map| object_fits(map, object)),
        JType::Union(items) => items.iter().any(|member| value_fits_type(value, member)),
    }
}

fn number_eq(actual: &Number, expected: &str) -> bool {
    if actual.to_string() == expected {
        return true;
    }
    match (actual.as_f64(), expected.parse::<f64>().ok()) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

fn object_fits(value: &Map<String, Value>, object: &ObjectType) -> bool {
    for (key, prop) in &object.properties {
        match value.get(key) {
            Some(actual) if !value_fits_type(actual, &prop.ty) => return false,
            Some(_) => {}
            None if prop.required => return false,
            None => {}
        }
    }
    if let Some(additional) = &object.additional {
        for (key, actual) in value {
            if object.properties.contains_key(key) {
                continue;
            }
            if !value_fits_type(actual, additional) {
                return false;
            }
        }
    } else {
        for key in value.keys() {
            if !object.properties.contains_key(key) {
                return false;
            }
        }
    }
    true
}

fn object_to_json_schema(object: &ObjectType) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();

    for (key, prop) in &object.properties {
        properties.insert(key.clone(), type_to_json_schema(&prop.ty));
        if prop.required {
            required.push(Value::String(key.clone()));
        }
    }

    let additional = match &object.additional {
        None => Value::Bool(false),
        Some(ty) if matches!(**ty, JType::Unknown) => Value::Bool(true),
        Some(ty) => type_to_json_schema(ty),
    };

    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": additional,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_basic_object_schema() {
        let schema = json!({
            "type": "object",
            "properties": {
                "id": { "type": "number" },
                "name": { "type": "string" }
            },
            "required": ["id"],
            "additionalProperties": false
        });

        assert_eq!(
            json_schema_to_type(&schema).to_compact_string(),
            "object{id: number, name?: string}"
        );
    }

    #[test]
    fn infers_sample_shape() {
        let sample = json!({"items": [{"name": "Ada"}, {"name": "Grace"}]});
        assert_eq!(
            sample_to_type(&sample).to_compact_string(),
            "object{items: array<object{name: \"Ada\"} | object{name: \"Grace\"}>}"
        );
    }

    #[test]
    fn value_fits_primitive_and_literal_types() {
        assert!(value_fits_type(&json!(null), &JType::Null));
        assert!(!value_fits_type(&json!(false), &JType::Null));

        assert!(value_fits_type(&json!(true), &JType::bool()));
        assert!(value_fits_type(&json!(true), &JType::bool_lit(true)));
        assert!(!value_fits_type(&json!(false), &JType::bool_lit(true)));

        assert!(value_fits_type(&json!(42), &JType::number()));
        assert!(value_fits_type(&json!(42), &JType::number_lit("42")));
        assert!(value_fits_type(&json!(42.0), &JType::number_lit("42")));
        assert!(!value_fits_type(&json!(43), &JType::number_lit("42")));

        assert!(value_fits_type(&json!("hi"), &JType::string()));
        assert!(value_fits_type(&json!("hi"), &JType::string_lit("hi")));
    }

    #[test]
    fn value_fits_arrays_objects_and_unions() {
        let array = JType::array(JType::string());
        assert!(value_fits_type(&json!(["a", "b"]), &array));
        assert!(!value_fits_type(&json!(["a", 1]), &array));

        let mut props = BTreeMap::new();
        props.insert(
            "id".to_string(),
            Property {
                ty: JType::number(),
                required: true,
            },
        );
        props.insert(
            "name".to_string(),
            Property {
                ty: JType::string(),
                required: false,
            },
        );
        let closed = JType::closed_object(props.clone());
        assert!(value_fits_type(&json!({"id": 1, "name": "Ada"}), &closed));
        assert!(value_fits_type(&json!({"id": 1}), &closed));
        assert!(!value_fits_type(&json!({"id": 1, "extra": true}), &closed));
        assert!(!value_fits_type(&json!({"name": "Ada"}), &closed));

        let open = JType::open_object(props);
        assert!(value_fits_type(&json!({"id": 1, "extra": true}), &open));

        let union = JType::union([JType::string(), JType::Null]);
        assert!(value_fits_type(&json!("x"), &union));
        assert!(value_fits_type(&json!(null), &union));
        assert!(!value_fits_type(&json!(1), &union));
    }

    #[test]
    fn unknown_accepts_any_value_never_rejects_all() {
        assert!(value_fits_type(&json!(null), &JType::Unknown));
        assert!(value_fits_type(&json!({"any": [1, 2]}), &JType::Unknown));
        assert!(!value_fits_type(&json!(null), &JType::Never));
    }
}
