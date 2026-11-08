//! JSON envelope: `{"ok":true,"data":…}` or `{"ok":false,"error":{"code","message"}}`.

use serde::Serialize;
use serde_json::{json, Value};
use std::fmt::Display;

use crate::shared::domain::error::ErrorCode;

/// A command error with a stable code. Messages never contain secrets.
#[derive(Debug)]
pub struct CliError {
    pub code: &'static str,
    pub message: String,
}

impl CliError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl<E: ErrorCode + Display> From<E> for CliError {
    fn from(e: E) -> Self {
        Self {
            code: e.code(),
            message: e.to_string(),
        }
    }
}

/// Successful result: JSON data plus its human rendering.
pub struct Reply {
    pub data: Value,
    pub human: String,
}

impl Reply {
    pub fn new<T: Serialize>(data: &T, human: impl Into<String>) -> Self {
        Self {
            data: serde_json::to_value(data).unwrap_or(Value::Null),
            human: human.into(),
        }
    }
}

pub fn ok_envelope(data: &Value) -> String {
    json!({"ok": true, "data": data}).to_string()
}

pub fn err_envelope(e: &CliError) -> String {
    json!({"ok": false, "error": {"code": e.code, "message": e.message}}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes_have_the_contract_shape() {
        assert_eq!(
            ok_envelope(&json!({"a": 1})),
            r#"{"ok":true,"data":{"a":1}}"#
        );
        assert_eq!(
            err_envelope(&CliError::new("not_found", "event not found")),
            r#"{"ok":false,"error":{"code":"not_found","message":"event not found"}}"#
        );
    }
}
