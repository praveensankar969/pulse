use serde::{Deserialize, Serialize};

use super::ValidationError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertOp {
    Equals,
    NotEquals,
    Contains,
    Exists,
    Gt,
    Lt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assertion {
    pub path: String,
    pub op: AssertOp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssertionResult {
    pub path: String,
    pub op: AssertOp,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Assertion {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.path.is_empty() || self.path.len() > 256 {
            return Err(ValidationError::AssertionPath);
        }
        if let Some(value) = &self.value {
            if value.is_object() || value.is_array() {
                let bytes =
                    serde_json::to_vec(value).map_err(|_| ValidationError::AssertionValue)?;
                if bytes.len() > 1024 {
                    return Err(ValidationError::AssertionValue);
                }
            } else if let Some(s) = value.as_str() {
                if s.chars().count() > 1024 {
                    return Err(ValidationError::AssertionValue);
                }
            }
        }
        Ok(())
    }
}
