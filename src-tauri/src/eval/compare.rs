use serde_json::Value;

use crate::domain::{
    AssertOp, AssertionResult, REASON_MISSING, REASON_NOT_CONTAINABLE, REASON_NOT_NUMERIC,
};

pub fn compare(op: AssertOp, actual: Option<&Value>, expected: Option<&Value>) -> AssertionResult {
    let mut result = AssertionResult {
        path: String::new(),
        op,
        ok: false,
        expected: expected.cloned(),
        actual: actual.cloned(),
        reason: None,
    };

    match op {
        AssertOp::Exists => {
            result.ok = actual.is_some();
            if !result.ok {
                result.reason = Some(REASON_MISSING.into());
            }
        }
        AssertOp::Equals => match actual {
            None => result.reason = Some(REASON_MISSING.into()),
            Some(actual) => {
                result.ok = json_eq(actual, expected.unwrap_or(&Value::Null));
            }
        },
        AssertOp::NotEquals => match actual {
            None => result.ok = true,
            Some(actual) => {
                result.ok = !json_eq(actual, expected.unwrap_or(&Value::Null));
            }
        },
        AssertOp::Contains => match actual {
            None => result.reason = Some(REASON_MISSING.into()),
            Some(actual) => match contains(actual, expected.unwrap_or(&Value::Null)) {
                Ok(ok) => result.ok = ok,
                Err(()) => result.reason = Some(REASON_NOT_CONTAINABLE.into()),
            },
        },
        AssertOp::Gt | AssertOp::Lt => match actual {
            None => result.reason = Some(REASON_MISSING.into()),
            Some(actual) => match (numeric_coerce(actual), expected.and_then(numeric_coerce)) {
                (Some(left), Some(right)) => {
                    result.ok = if op == AssertOp::Gt {
                        left > right
                    } else {
                        left < right
                    };
                }
                _ => result.reason = Some(REASON_NOT_NUMERIC.into()),
            },
        },
    }

    result
}

fn json_eq(actual: &Value, expected: &Value) -> bool {
    if same_json_type(actual, expected) {
        return structurally_eq(actual, expected);
    }
    // One-way coerce of actual toward expected's type; never the reverse.
    structurally_eq(&coerce_toward(actual, expected), expected)
}

fn structurally_eq(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(l, r)| structurally_eq(l, r))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, v)| b.get(k).is_some_and(|other| structurally_eq(v, other)))
        }
        _ => left == right,
    }
}

fn same_json_type(left: &Value, right: &Value) -> bool {
    matches!(
        (left, right),
        (Value::Null, Value::Null)
            | (Value::Bool(_), Value::Bool(_))
            | (Value::Number(_), Value::Number(_))
            | (Value::String(_), Value::String(_))
            | (Value::Array(_), Value::Array(_))
            | (Value::Object(_), Value::Object(_))
    )
}

fn coerce_toward(actual: &Value, expected: &Value) -> Value {
    match expected {
        Value::Bool(_) => {
            if let Some(s) = actual.as_str() {
                if s.eq_ignore_ascii_case("true") {
                    return Value::Bool(true);
                }
                if s.eq_ignore_ascii_case("false") {
                    return Value::Bool(false);
                }
            }
            if let Some(n) = actual.as_f64() {
                if n == 1.0 {
                    return Value::Bool(true);
                }
                if n == 0.0 {
                    return Value::Bool(false);
                }
            }
            actual.clone()
        }
        Value::Number(_) => {
            if let Some(s) = actual.as_str() {
                if let Some(n) = parse_finite_f64(s) {
                    return n;
                }
            }
            if let Some(b) = actual.as_bool() {
                return Value::from(u64::from(b));
            }
            actual.clone()
        }
        Value::String(_) => match actual {
            Value::Number(n) => Value::String(n.to_string()),
            Value::Bool(b) => Value::String(b.to_string()),
            Value::Null => Value::String("null".into()),
            _ => actual.clone(),
        },
        _ => actual.clone(),
    }
}

fn contains(actual: &Value, expected: &Value) -> Result<bool, ()> {
    match actual {
        Value::String(text) => Ok(text.contains(&stringify_value(expected))),
        Value::Array(items) => Ok(items.iter().any(|item| json_eq(item, expected))),
        Value::Object(obj) => match expected.as_str() {
            Some(key) => Ok(obj.contains_key(key)),
            None => Err(()),
        },
        _ => Err(()),
    }
}

fn numeric_coerce(value: &Value) -> Option<f64> {
    if let Some(n) = value.as_f64() {
        return n.is_finite().then_some(n);
    }
    if let Some(s) = value.as_str() {
        let n = s.parse::<f64>().ok()?;
        return n.is_finite().then_some(n);
    }
    value.as_bool().map(u8::from).map(f64::from)
}

fn parse_finite_f64(s: &str) -> Option<Value> {
    let n = s.parse::<f64>().ok()?;
    serde_json::Number::from_f64(n)
        .filter(|_| n.is_finite())
        .map(Value::Number)
}

pub(crate) fn stringify_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".into(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exists_null_counts() {
        let r = compare(AssertOp::Exists, Some(&Value::Null), None);
        assert!(r.ok);
        let r = compare(AssertOp::Exists, None, None);
        assert!(!r.ok);
        assert_eq!(r.reason.as_deref(), Some(REASON_MISSING));
    }

    #[test]
    fn missing_not_equals_passes() {
        let r = compare(AssertOp::NotEquals, None, Some(&json!("x")));
        assert!(r.ok);
        let r = compare(AssertOp::Equals, None, Some(&json!("x")));
        assert!(!r.ok);
        assert_eq!(r.reason.as_deref(), Some(REASON_MISSING));
    }

    #[test]
    fn coerce_table() {
        assert!(compare(AssertOp::Equals, Some(&json!("true")), Some(&json!(true))).ok);
        assert!(compare(AssertOp::Equals, Some(&json!("FALSE")), Some(&json!(false))).ok);
        assert!(compare(AssertOp::Equals, Some(&json!(1)), Some(&json!(true))).ok);
        assert!(compare(AssertOp::Equals, Some(&json!(0)), Some(&json!(false))).ok);
        assert!(!compare(AssertOp::Equals, Some(&json!(2)), Some(&json!(true))).ok);
        assert!(compare(AssertOp::Equals, Some(&json!("1.5")), Some(&json!(1.5))).ok);
        assert!(compare(AssertOp::Equals, Some(&json!(true)), Some(&json!(1))).ok);
        assert!(compare(AssertOp::Equals, Some(&json!(1)), Some(&json!("1"))).ok);
        assert!(compare(AssertOp::Equals, Some(&Value::Null), Some(&json!("null"))).ok);
        assert!(
            !compare(
                AssertOp::Equals,
                Some(&json!({"a":1})),
                Some(&json!({"a":2}))
            )
            .ok
        );
        assert!(compare(AssertOp::Equals, Some(&json!([])), Some(&json!([]))).ok);
        assert!(compare(AssertOp::Equals, Some(&json!(1.0)), Some(&json!(1))).ok);
        assert!(!compare(AssertOp::NotEquals, Some(&json!("1")), Some(&json!(1))).ok);
    }

    #[test]
    fn contains_and_numeric() {
        assert!(
            compare(
                AssertOp::Contains,
                Some(&json!("hello")),
                Some(&json!("ell"))
            )
            .ok
        );
        assert!(
            compare(
                AssertOp::Contains,
                Some(&json!([1, "x"])),
                Some(&json!("x"))
            )
            .ok
        );
        assert!(compare(AssertOp::Contains, Some(&json!({"k":1})), Some(&json!("k"))).ok);
        let r = compare(AssertOp::Contains, Some(&json!(1)), Some(&json!("1")));
        assert!(!r.ok);
        assert_eq!(r.reason.as_deref(), Some(REASON_NOT_CONTAINABLE));
        assert!(compare(AssertOp::Gt, Some(&json!("10")), Some(&json!(3))).ok);
        assert!(compare(AssertOp::Lt, Some(&json!(true)), Some(&json!(2))).ok);
        let r = compare(AssertOp::Gt, Some(&json!("nope")), Some(&json!(1)));
        assert!(!r.ok);
        assert_eq!(r.reason.as_deref(), Some(REASON_NOT_NUMERIC));
        assert!(!compare(AssertOp::Gt, Some(&json!(5)), Some(&json!(5))).ok);
    }

    #[test]
    fn minus_zero_equals_zero() {
        let neg = Value::Number(serde_json::Number::from_f64(-0.0).unwrap());
        let pos = json!(0.0);
        assert!(compare(AssertOp::Equals, Some(&neg), Some(&pos)).ok);
    }
}
