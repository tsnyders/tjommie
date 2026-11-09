//! JSON-RPC over newline-delimited JSON, as spoken by `codex app-server` (it omits the "jsonrpc" field).

use std::fmt;

use serde_json::{Number, Value};

use super::js;

/// A request id: a number or a string. Tjommie's own requests use numbers.
#[derive(Debug, Clone, PartialEq)]
pub enum RpcId {
    Number(Number),
    String(String),
}

impl RpcId {
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Number(number) => Some(RpcId::Number(number.clone())),
            Value::String(text) => Some(RpcId::String(text.clone())),
            _ => None,
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            RpcId::Number(number) => Value::Number(number.clone()),
            RpcId::String(text) => Value::String(text.clone()),
        }
    }
}

impl From<u64> for RpcId {
    fn from(id: u64) -> Self {
        RpcId::Number(id.into())
    }
}

impl From<&str> for RpcId {
    fn from(id: &str) -> Self {
        RpcId::String(id.into())
    }
}

/// `String(id)`.
impl fmt::Display for RpcId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&js::string(&self.to_value()))
    }
}
