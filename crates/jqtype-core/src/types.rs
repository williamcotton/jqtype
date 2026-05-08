use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BoolType {
    Any,
    Literal(bool),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NumberType {
    Any,
    Literal(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StringType {
    Any,
    Literal(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArrayType {
    pub items: Box<JType>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Property {
    pub ty: JType,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectType {
    pub properties: BTreeMap<String, Property>,
    pub additional: Option<Box<JType>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JType {
    Never,
    Unknown,
    Null,
    Bool(BoolType),
    Number(NumberType),
    String(StringType),
    Array(ArrayType),
    Object(ObjectType),
    Union(Vec<JType>),
}

impl JType {
    pub fn is_never(&self) -> bool {
        matches!(self, Self::Never)
    }

    pub fn bool() -> Self {
        Self::Bool(BoolType::Any)
    }

    pub fn bool_lit(value: bool) -> Self {
        Self::Bool(BoolType::Literal(value))
    }

    pub fn number() -> Self {
        Self::Number(NumberType::Any)
    }

    pub fn number_lit(value: impl Into<String>) -> Self {
        Self::Number(NumberType::Literal(value.into()))
    }

    pub fn string() -> Self {
        Self::String(StringType::Any)
    }

    pub fn string_lit(value: impl Into<String>) -> Self {
        Self::String(StringType::Literal(value.into()))
    }

    pub fn array(items: JType) -> Self {
        Self::Array(ArrayType {
            items: Box::new(items),
        })
    }

    pub fn object(properties: BTreeMap<String, Property>, additional: Option<JType>) -> Self {
        Self::Object(ObjectType {
            properties,
            additional: additional.map(Box::new),
        })
    }

    pub fn closed_object(properties: BTreeMap<String, Property>) -> Self {
        Self::object(properties, None)
    }

    pub fn open_object(properties: BTreeMap<String, Property>) -> Self {
        Self::object(properties, Some(Self::Unknown))
    }

    pub fn property(ty: JType, required: bool) -> Property {
        Property { ty, required }
    }

    pub fn union(items: impl IntoIterator<Item = JType>) -> Self {
        let mut out = Vec::new();
        for item in items {
            match item {
                JType::Never => {}
                JType::Unknown => return JType::Unknown,
                JType::Union(members) => out.extend(members),
                other => out.push(other),
            }
        }

        if out.is_empty() {
            return JType::Never;
        }

        let mut seen = BTreeSet::new();
        out.retain(|item| seen.insert(item.to_compact_string()));
        out.sort_by_key(|item| item.to_compact_string());

        if out.len() == 1 {
            out.pop().unwrap()
        } else {
            JType::Union(out)
        }
    }

    pub fn is_truthy_literal(&self) -> Option<bool> {
        match self {
            JType::Null => Some(false),
            JType::Bool(BoolType::Literal(false)) => Some(false),
            JType::Bool(BoolType::Literal(true)) => Some(true),
            JType::Bool(BoolType::Any) | JType::Unknown | JType::Union(_) => None,
            JType::Never => Some(false),
            _ => Some(true),
        }
    }

    pub fn type_names(&self) -> Vec<String> {
        match self {
            JType::Never => vec![],
            JType::Unknown => ["null", "boolean", "number", "string", "array", "object"]
                .into_iter()
                .map(String::from)
                .collect(),
            JType::Null => vec!["null".to_string()],
            JType::Bool(_) => vec!["boolean".to_string()],
            JType::Number(_) => vec!["number".to_string()],
            JType::String(_) => vec!["string".to_string()],
            JType::Array(_) => vec!["array".to_string()],
            JType::Object(_) => vec!["object".to_string()],
            JType::Union(items) => {
                let mut set = BTreeSet::new();
                for item in items {
                    set.extend(item.type_names());
                }
                set.into_iter().collect()
            }
        }
    }

    pub fn without_null(self) -> Self {
        match self {
            JType::Null => JType::Never,
            JType::Union(items) => JType::union(
                items
                    .into_iter()
                    .map(JType::without_null)
                    .filter(|item| !item.is_never()),
            ),
            other => other,
        }
    }

    pub fn to_compact_string(&self) -> String {
        match self {
            JType::Never => "empty".to_string(),
            JType::Unknown => "unknown".to_string(),
            JType::Null => "null".to_string(),
            JType::Bool(BoolType::Any) => "boolean".to_string(),
            JType::Bool(BoolType::Literal(value)) => value.to_string(),
            JType::Number(NumberType::Any) => "number".to_string(),
            JType::Number(NumberType::Literal(value)) => value.clone(),
            JType::String(StringType::Any) => "string".to_string(),
            JType::String(StringType::Literal(value)) => format!("{value:?}"),
            JType::Array(array) => format!("array<{}>", array.items.to_compact_string()),
            JType::Object(object) => object.to_compact_string(),
            JType::Union(items) => items
                .iter()
                .map(JType::to_compact_string)
                .collect::<Vec<_>>()
                .join(" | "),
        }
    }
}

impl ObjectType {
    pub fn to_compact_string(&self) -> String {
        let mut parts = Vec::new();
        for (key, prop) in &self.properties {
            let suffix = if prop.required { "" } else { "?" };
            parts.push(format!("{key}{suffix}: {}", prop.ty.to_compact_string()));
        }
        if let Some(additional) = &self.additional {
            if matches!(**additional, JType::Unknown) {
                parts.push("...".to_string());
            } else {
                parts.push(format!("...: {}", additional.to_compact_string()));
            }
        }
        format!("object{{{}}}", parts.join(", "))
    }
}
