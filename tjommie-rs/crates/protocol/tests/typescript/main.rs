//! `packages/protocol/src/*.test.ts`, ported one for one with the same fixtures. `production-relay-guard.test.ts` has no
//! port: it guards vitest's own network access. JSON cannot carry `Infinity` or `NaN`; `JSON.stringify` sends them as `null`,
//! so those cases are ported as `null`.

mod accounts;
mod attachments;
mod browser;
mod diagnostics;
mod google;
mod mentions;
mod model_data;
mod native_browser;
mod projects;
mod relay;
mod remote;
mod vault;
mod wire;

use protocol::{DecodeError, Request, validate};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// `methods[method].params.parse(params)`, as the JSON the decoded params serialize back to.
pub fn params(method: &str, params: Value) -> Value {
    let request =
        Request::decode(method, params).unwrap_or_else(|error| panic!("{method}: {error}"));
    serde_json::to_value(request).unwrap()["params"].take()
}

/// `methods[method].params.safeParse(params).success === false`
pub fn rejects(method: &str, params: Value) {
    let decoded = Request::decode(method, params.clone());
    assert!(
        matches!(decoded, Err(DecodeError::BadParams(_))),
        "{method} accepted {params}: {decoded:?}"
    );
}

/// A result or event payload is valid and survives a serde round trip.
pub fn round_trip<T: Serialize + DeserializeOwned>(name: &str, value: Value) -> T {
    protocol::assert_valid(name, &value);
    let typed: T =
        serde_json::from_value(value.clone()).unwrap_or_else(|error| panic!("{name}: {error}"));
    let back = serde_json::to_value(&typed).unwrap();
    assert!(
        same(&back, &value),
        "{name} round trip\n  sent: {value}\n  back: {back}"
    );
    typed
}

/// `events[name].safeParse(value).success === false`, or the same for a method's result.
pub fn invalid(name: &str, value: Value) {
    assert!(validate(name, &value).is_err(), "{name} accepted {value}");
}

/// `{ ...base, ...extra }`
pub fn with(mut base: Value, extra: Value) -> Value {
    for (key, value) in extra.as_object().unwrap() {
        base[key] = value.clone();
    }
    base
}

/// `{ ...base, [key]: undefined }`
pub fn without(mut base: Value, key: &str) -> Value {
    base.as_object_mut().unwrap().remove(key);
    base
}

/// JSON equality where `0` and `0.0` are the same number, as they are in JavaScript.
pub fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| same(value, other)))
        }
        _ => a == b,
    }
}
