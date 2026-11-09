//! `manifest.json` as validators. Incoming values are trimmed where zod trims, then checked against the JSON Schema.

use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::wire::DecodeError;

const MANIFEST: &str = include_str!("../../../protocol/manifest.json");

/// A value that broke the protocol contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid(pub String);

impl fmt::Display for Invalid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Invalid {}

/// A standalone schema from the manifest's `types`, decoded with [`parse`].
pub trait Wire: DeserializeOwned {
    const NAME: &'static str;
}

/// Decodes a standalone type the way its zod schema's `parse` would.
pub fn parse<T: Wire>(value: Value) -> Result<T, Invalid> {
    let schema = contract()
        .types
        .get(T::NAME)
        .unwrap_or_else(|| panic!("{} is not in manifest.json", T::NAME));
    let value = schema.decode(value)?;
    serde_json::from_value(value).map_err(|error| Invalid(error.to_string()))
}

/// Checks a method's result, or an event's data, against the manifest.
pub fn validate(name: &str, value: &Value) -> Result<(), Invalid> {
    let schema = contract()
        .sent
        .get(name)
        .ok_or_else(|| Invalid(format!("{name} is not a method or event")))?;
    schema.check(value)
}

/// Panics unless `value` is a valid result of the method `name`, or valid data for the event `name`. For tests of what the
/// daemon sends.
#[track_caller]
pub fn assert_valid(name: &str, value: &Value) {
    if let Err(error) = validate(name, value) {
        panic!("{name}: {error}\n{value}");
    }
}

pub(crate) fn decode_params(method: &str, params: Value) -> Result<Value, DecodeError> {
    let schema = contract()
        .params
        .get(method)
        .ok_or(DecodeError::UnknownMethod)?;
    schema
        .decode(params)
        .map_err(|Invalid(message)| DecodeError::BadParams(message))
}

/// `String.prototype.trim`: Unicode White_Space without U+0085, plus U+FEFF.
pub(crate) fn js_trim(text: &str) -> &str {
    text.trim_matches(|c: char| c == '\u{feff}' || (c.is_whitespace() && c != '\u{85}'))
}

struct Schema {
    json: Value,
    validator: OnceLock<Option<jsonschema::Validator>>,
}

impl Schema {
    fn decode(&self, mut value: Value) -> Result<Value, Invalid> {
        trim(&self.json, &mut value);
        self.check(&value)?;
        Ok(value)
    }

    /// `"custom"` schemas (none today) pass here; their Rust types carry hand-written checks.
    fn check(&self, value: &Value) -> Result<(), Invalid> {
        let validator = self.validator.get_or_init(|| match &self.json {
            Value::String(custom) if custom == "custom" => None,
            json => Some(jsonschema::validator_for(json).expect("every manifest schema compiles")),
        });
        match validator
            .as_ref()
            .and_then(|validator| validator.iter_errors(value).next())
        {
            Some(error) => Err(Invalid(format!("{} at {}", error, error.instance_path()))),
            None => Ok(()),
        }
    }
}

/// Zod trims before it checks lengths; the manifest marks those strings `"x-trim": true`. A value inside a union is trimmed
/// when any branch trims it.
/// ponytail: right while no two union branches disagree about trimming the same key; walk only the matching branch if one does.
fn trim(schema: &Value, value: &mut Value) {
    if schema.get("x-trim") == Some(&Value::Bool(true))
        && let Value::String(text) = value
    {
        let trimmed = js_trim(text);
        if trimmed.len() != text.len() {
            *text = trimmed.to_owned();
        }
    }
    match value {
        Value::Object(fields) => {
            if let Some(Value::Object(properties)) = schema.get("properties") {
                for (key, field) in fields.iter_mut() {
                    if let Some(property) = properties.get(key) {
                        trim(property, field);
                    }
                }
            }
        }
        Value::Array(items) => {
            if let Some(item) = schema.get("items") {
                items.iter_mut().for_each(|value| trim(item, value));
            }
        }
        _ => {}
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(Value::Array(branches)) = schema.get(key) {
            branches.iter().for_each(|branch| trim(branch, value));
        }
    }
}

#[derive(Default)]
struct Contract {
    params: HashMap<String, Schema>,
    /// Method results and event data: what the daemon sends.
    sent: HashMap<String, Schema>,
    types: HashMap<String, Schema>,
}

fn contract() -> &'static Contract {
    static CONTRACT: OnceLock<Contract> = OnceLock::new();
    CONTRACT.get_or_init(|| {
        #[derive(Deserialize)]
        struct Manifest {
            methods: Vec<MethodEntry>,
            events: Vec<EventEntry>,
            types: Vec<TypeEntry>,
        }
        #[derive(Deserialize)]
        struct MethodEntry {
            name: String,
            params: Value,
            result: Value,
        }
        #[derive(Deserialize)]
        struct EventEntry {
            name: String,
            data: Value,
        }
        #[derive(Deserialize)]
        struct TypeEntry {
            name: String,
            schema: Value,
        }
        let schema = |json| Schema {
            json,
            validator: OnceLock::new(),
        };
        let manifest: Manifest = serde_json::from_str(MANIFEST).expect("manifest.json parses");
        let mut contract = Contract::default();
        for method in manifest.methods {
            contract
                .params
                .insert(method.name.clone(), schema(method.params));
            contract.sent.insert(method.name, schema(method.result));
        }
        for event in manifest.events {
            contract.sent.insert(event.name, schema(event.data));
        }
        for entry in manifest.types {
            contract.types.insert(entry.name, schema(entry.schema));
        }
        contract
    })
}
