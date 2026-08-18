use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use pulse_lib::domain::{
    AssertOp, Assertion, ExpectedStatus, HttpMethod, OutcomeClass, Service, ServiceStatus,
};
use pulse_lib::eval::{
    compare, evaluate_at, outcome_of, parse_path, resolve_path, Outcome, PathError, RawResponse,
    TransportError,
};
use serde_json::{json, Value};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/eval")
}

fn load(name: &str) -> Value {
    let path = fixtures_dir().join(name);
    serde_json::from_slice(&fs::read(&path).unwrap_or_else(|error| {
        panic!("failed to read {}: {error}", path.display());
    }))
    .unwrap()
}

fn fixed_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-08-18T14:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn sample_service() -> Service {
    Service {
        id: "01TEST00000000000000000000".into(),
        name: "t".into(),
        url: "https://example.com/health".into(),
        method: HttpMethod::Get,
        headers: vec![],
        body: None,
        interval_sec: 60,
        timeout_ms: 10_000,
        expected_status: ExpectedStatus::TwoXx,
        assertions: vec![],
        max_latency_ms: None,
        action_url: None,
        notify: true,
        always_alert: false,
        paused: false,
        follow_redirects: true,
        fail_threshold: None,
        group: None,
        created_at: fixed_time(),
        updated_at: fixed_time(),
    }
}

#[test]
fn path_fixtures() {
    let doc = load("paths.json");
    for case in doc["parse"].as_array().unwrap() {
        let path = case["path"].as_str().unwrap();
        let ok = case["ok"].as_bool().unwrap();
        assert_eq!(
            parse_path(path).is_ok(),
            ok,
            "parse {path:?} expected ok={ok}"
        );
    }

    let root = &doc["root"];
    assert_eq!(resolve_path(root, "$").unwrap().as_ref(), root);

    for case in doc["resolve"].as_array().unwrap() {
        let path = case["path"].as_str().unwrap();
        match resolve_path(root, path) {
            Ok(value) => {
                assert!(
                    case.get("value").is_some(),
                    "{path} resolved but fixture expected error"
                );
                assert_eq!(value.as_ref(), &case["value"], "resolve {path}");
            }
            Err(err) => {
                let expected = case["error"].as_str().unwrap();
                let got = match err {
                    PathError::Invalid => "invalid_path",
                    PathError::Missing => "missing",
                };
                assert_eq!(got, expected, "resolve {path}");
            }
        }
    }
}

#[test]
fn compare_fixtures() {
    for case in load("compare.json").as_array().unwrap() {
        let op: AssertOp = serde_json::from_value(case["op"].clone()).unwrap();
        let actual = if case["missing"].as_bool().unwrap_or(false) {
            None
        } else {
            case.get("actual")
        };
        let expected = case.get("expected");
        let result = compare(op, actual, expected);
        assert_eq!(result.ok, case["ok"].as_bool().unwrap(), "case {case}");
        match case.get("reason") {
            Some(reason) => assert_eq!(result.reason.as_deref(), reason.as_str(), "reason {case}"),
            None => assert_eq!(result.reason, None, "reason {case}"),
        }
    }
}

#[test]
fn classify_matrix_fixtures() {
    for case in load("classify.json").as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let service = service_from_case(case);
        let raw = raw_from_case(case);
        let evidence = evaluate_at(&service, raw, fixed_time());

        let outcome: OutcomeClass = serde_json::from_value(case["outcome"].clone()).unwrap();
        assert_eq!(evidence.outcome, outcome, "{name} outcome");

        match case.get("errorKind") {
            Some(kind) => {
                let expected: pulse_lib::domain::ErrorKind =
                    serde_json::from_value(kind.clone()).unwrap();
                assert_eq!(evidence.error_kind, Some(expected), "{name} errorKind");
            }
            None => assert_eq!(evidence.error_kind, None, "{name} errorKind"),
        }

        if let Some(error) = case["error"].as_str() {
            assert_eq!(evidence.error.as_deref(), Some(error), "{name} error");
        }
        if case["errorUnset"].as_bool().unwrap_or(false) {
            assert_eq!(evidence.error, None, "{name} error must be unset");
        }
        if let Some(skipped) = case.get("assertionSkipped") {
            let expected: pulse_lib::domain::AssertionSkipped =
                serde_json::from_value(skipped.clone()).unwrap();
            assert_eq!(evidence.assertion_skipped, Some(expected), "{name} skip");
            assert!(
                evidence.assertion_results.is_empty(),
                "{name} assertionResults"
            );
        }

        match evidence.outcome {
            OutcomeClass::Ok => {
                assert!(matches!(outcome_of(&evidence), Outcome::Success { .. }));
            }
            OutcomeClass::Soft => {
                assert!(matches!(outcome_of(&evidence), Outcome::SoftFail { .. }));
            }
            OutcomeClass::Hard => {
                assert!(matches!(outcome_of(&evidence), Outcome::HardFail { .. }));
            }
        }
    }
}

#[test]
fn evaluate_never_emits_service_status() {
    let encoded = serde_json::to_value(evaluate_at(
        &sample_service(),
        Ok(RawResponse {
            status: 200,
            latency_ms: 10,
            redirects: 0,
            headers_stripped_on_redirect: false,
            body: b"{}".to_vec(),
            body_preview: "{}".into(),
        }),
        fixed_time(),
    ))
    .unwrap();
    assert!(encoded.get("state").is_none());
    let _not_a_status: Result<ServiceStatus, _> =
        serde_json::from_value(encoded["outcome"].clone());
    assert!(serde_json::to_value(ServiceStatus::Healthy).unwrap() != encoded["outcome"]);
}

fn service_from_case(case: &Value) -> Service {
    let mut service = sample_service();
    if let Some(method) = case["method"].as_str() {
        service.method = match method {
            "HEAD" => HttpMethod::Head,
            "POST" => HttpMethod::Post,
            _ => HttpMethod::Get,
        };
    }
    if let Some(status) = case.get("expectedStatus") {
        service.expected_status = serde_json::from_value(status.clone()).unwrap();
    }
    if let Some(assertions) = case.get("assertions") {
        service.assertions = serde_json::from_value::<Vec<Assertion>>(assertions.clone()).unwrap();
    }
    if let Some(max) = case["maxLatencyMs"].as_u64() {
        service.max_latency_ms = Some(max as u32);
    }
    if let Some(follow) = case["followRedirects"].as_bool() {
        service.follow_redirects = follow;
    }
    if let Some(timeout) = case["timeoutMs"].as_u64() {
        service.timeout_ms = timeout as u32;
    }
    service
}

fn raw_from_case(case: &Value) -> Result<RawResponse, TransportError> {
    if let Some(kind) = case.get("transport") {
        return Err(TransportError {
            kind: serde_json::from_value(kind.clone()).unwrap(),
            latency_ms: case["transportLatencyMs"].as_u64().map(|n| n as u32),
            detail: case["transportDetail"].as_str().map(str::to_owned),
        });
    }
    let response = &case["response"];
    let (body, preview) = if let Some(text) = response["bodyText"].as_str() {
        (text.as_bytes().to_vec(), text.to_string())
    } else if response.get("body").is_none() || response["body"].is_null() {
        (Vec::new(), String::new())
    } else {
        let bytes = serde_json::to_vec(&response["body"]).unwrap();
        let preview = String::from_utf8_lossy(&bytes).into_owned();
        (bytes, preview)
    };
    Ok(RawResponse {
        status: response["status"].as_u64().unwrap() as u16,
        latency_ms: response["latencyMs"].as_u64().unwrap_or(10) as u32,
        redirects: response["redirects"].as_u64().unwrap_or(0) as u8,
        headers_stripped_on_redirect: response["headersStrippedOnRedirect"]
            .as_bool()
            .unwrap_or(false),
        body,
        body_preview: preview,
    })
}

#[test]
fn dollar_root_equals_whole_document() {
    let root = json!({"status":"ok","errors":[]});
    let service = {
        let mut s = sample_service();
        s.assertions = vec![Assertion {
            path: "$".into(),
            op: AssertOp::Equals,
            value: Some(root.clone()),
        }];
        s
    };
    let body = serde_json::to_vec(&root).unwrap();
    let evidence = evaluate_at(
        &service,
        Ok(RawResponse {
            status: 200,
            latency_ms: 10,
            redirects: 0,
            headers_stripped_on_redirect: false,
            body,
            body_preview: root.to_string(),
        }),
        fixed_time(),
    );
    assert_eq!(evidence.outcome, OutcomeClass::Ok);
    assert!(evidence.assertion_results[0].ok);
}
