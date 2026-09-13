//! Portable data contracts shared by native host and Wasm guests.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_BYTES: usize = 512 * 1024;
pub const MAX_NODES: usize = 4096;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Issue {
    pub id: i64,
    pub title: String,
    pub completed_day: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Node {
    Column {
        key: String,
        children: Vec<Node>,
    },
    Row {
        key: String,
        children: Vec<Node>,
    },
    Text {
        key: String,
        text: String,
    },
    Button {
        #[serde(default)]
        name: Option<String>,
        key: String,
        label: String,
        action: String,
    },
    Input {
        key: String,
        label: String,
        value: String,
        revision: u64,
        action: String,
    },
}
impl Node {
    pub fn key(&self) -> &str {
        match self {
            Self::Column { key, .. }
            | Self::Row { key, .. }
            | Self::Text { key, .. }
            | Self::Button { key, .. }
            | Self::Input { key, .. } => key,
        }
    }
    pub fn validate(&self) -> Result<()> {
        fn visit(node: &Node, depth: usize, keys: &mut BTreeSet<String>) -> Result<()> {
            ensure!(depth <= 32, "UI nesting limit");
            ensure!(keys.len() < MAX_NODES, "UI node limit");
            ensure!(keys.insert(node.key().into()), "duplicate node key");
            ensure!(
                !node.key().is_empty() && node.key().len() <= 128,
                "invalid node key"
            );
            if let Node::Column { children, .. } | Node::Row { children, .. } = node {
                for child in children {
                    visit(child, depth + 1, keys)?;
                }
            }
            Ok(())
        }
        visit(self, 0, &mut BTreeSet::new())
    }
}
pub fn decode_ui(encoded: &str) -> Result<Node> {
    ensure!(encoded.len() <= MAX_BYTES, "UI byte limit");
    let node: Node = serde_json::from_str(encoded)?;
    node.validate()?;
    Ok(node)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub value: Option<String>,
    pub revision: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenderInput {
    pub issues: Vec<Issue>,
    pub selected_day: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_identity_is_rejected() {
        let n = Node::Column {
            key: "same".into(),
            children: vec![Node::Text {
                key: "same".into(),
                text: "x".into(),
            }],
        };
        assert!(n.validate().is_err());
    }
}

/// Versioned, deliberately small schema language; this is not all of JSON Schema.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Schema {
    Null,
    Boolean,
    Integer,
    String,
    Nullable {
        value: Box<Schema>,
    },
    Array {
        items: Box<Schema>,
    },
    Object {
        fields: std::collections::BTreeMap<String, Schema>,
    },
}
impl Schema {
    pub fn check(&self, value: &serde_json::Value) -> Result<()> {
        use serde_json::Value;
        match (self, value) {
            (Self::Null, Value::Null)
            | (Self::Boolean, Value::Bool(_))
            | (Self::String, Value::String(_)) => Ok(()),
            (Self::Integer, Value::Number(n)) if n.is_i64() || n.is_u64() => Ok(()),
            (Self::Nullable { .. }, Value::Null) => Ok(()),
            (Self::Nullable { value: schema }, value) => schema.check(value),
            (Self::Array { items }, Value::Array(values)) => {
                for value in values {
                    items.check(value)?;
                }
                Ok(())
            }
            (Self::Object { fields }, Value::Object(values)) => {
                ensure!(
                    fields.len() == values.len(),
                    "object fields differ from contract"
                );
                for (key, schema) in fields {
                    schema.check(
                        values
                            .get(key)
                            .ok_or_else(|| anyhow::anyhow!("missing field {key}"))?,
                    )?;
                }
                Ok(())
            }
            _ => anyhow::bail!("value does not match schema"),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Method {
    pub input: Schema,
    pub output: Schema,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    pub version: u32,
    pub methods: std::collections::BTreeMap<String, Method>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub abi: u32,
    pub id: String,
    pub requires: std::collections::BTreeMap<String, Contract>,
    pub provides: std::collections::BTreeMap<String, Contract>,
    pub capabilities: Vec<String>,
    pub view_query: String,
}
