use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::domain::{
    Assertion, AssertionResult, AssertionSkipped, CheckEvidence, ErrorKind, ExpectedStatus,
    HttpMethod, MessageArgs, OutcomeClass, Service, REASON_INVALID_PATH,
};

use super::compare::{compare, stringify_value};
use super::path::{resolve_path, PathError};
use super::{Outcome, RawResponse, TransportError};

pub fn evaluate(service: &Service, raw: Result<RawResponse, TransportError>) -> CheckEvidence {
    evaluate_at(service, raw, Utc::now())
}

pub fn evaluate_at(
    service: &Service,
    raw: Result<RawResponse, TransportError>,
    at: DateTime<Utc>,
) -> CheckEvidence {
    match raw {
        Err(err) => transport_hard(service, err, at),
        Ok(raw) => evaluate_response(service, raw, at),
    }
}

pub fn outcome_of(evidence: &CheckEvidence) -> Outcome {
    match evidence.outcome {
        OutcomeClass::Ok => Outcome::Success {
            http_status: evidence.http_status.unwrap_or(0),
            latency_ms: evidence.latency_ms.unwrap_or(0),
            redirects: evidence.redirects.unwrap_or(0),
        },
        OutcomeClass::Soft => Outcome::SoftFail {
            kind: evidence.error_kind.unwrap_or(ErrorKind::Slow),
            http_status: evidence.http_status.unwrap_or(0),
            latency_ms: evidence.latency_ms.unwrap_or(0),
        },
        OutcomeClass::Hard => Outcome::HardFail {
            kind: evidence.error_kind.unwrap_or(ErrorKind::Assertion),
            http_status: evidence.http_status,
            latency_ms: evidence.latency_ms,
        },
    }
}

fn transport_hard(service: &Service, err: TransportError, at: DateTime<Utc>) -> CheckEvidence {
    let args = MessageArgs {
        timeout_ms: Some(service.timeout_ms),
        secret_key: err.detail.as_deref(),
        ..MessageArgs::default()
    };
    let message = err.kind.user_message(&args);
    CheckEvidence {
        at,
        outcome: OutcomeClass::Hard,
        http_status: None,
        latency_ms: err.latency_ms,
        redirects: None,
        headers_stripped_on_redirect: None,
        assertion_results: Vec::new(),
        assertion_skipped: None,
        error_kind: Some(err.kind),
        error: nonempty(message),
        body_preview: None,
    }
}

fn evaluate_response(service: &Service, raw: RawResponse, at: DateTime<Utc>) -> CheckEvidence {
    let mut evidence = CheckEvidence {
        at,
        outcome: OutcomeClass::Ok,
        http_status: Some(raw.status),
        latency_ms: Some(raw.latency_ms),
        redirects: Some(raw.redirects),
        headers_stripped_on_redirect: raw.headers_stripped_on_redirect.then_some(true),
        assertion_results: Vec::new(),
        assertion_skipped: None,
        error_kind: None,
        error: None,
        body_preview: preview(&raw.body_preview),
    };

    if !status_matches(&service.expected_status, raw.status) {
        evidence.outcome = OutcomeClass::Hard;
        evidence.error_kind = Some(ErrorKind::UnexpectedStatus);
        evidence.error = Some(ErrorKind::UnexpectedStatus.user_message(&MessageArgs {
            http_status: Some(raw.status),
            ..MessageArgs::default()
        }));
        return evidence;
    }

    if service.method == HttpMethod::Head {
        if !service.assertions.is_empty() {
            evidence.assertion_skipped = Some(AssertionSkipped::Head);
        }
    } else if !service.assertions.is_empty() {
        // 204 has no body; assertions cannot be evaluated.
        if raw.status == 204 {
            return body_parse(evidence);
        }
        let parsed = match serde_json::from_slice::<Value>(&raw.body) {
            Ok(value) => value,
            Err(_) => return body_parse(evidence),
        };
        evidence.assertion_results = service
            .assertions
            .iter()
            .map(|assertion| eval_assertion(assertion, &parsed))
            .collect();
        if let Some(fail) = evidence.assertion_results.iter().find(|r| !r.ok) {
            evidence.outcome = OutcomeClass::Hard;
            evidence.error_kind = Some(ErrorKind::Assertion);
            evidence.error = Some(assertion_message(fail));
            return evidence;
        }
    }

    if let Some(max) = service.max_latency_ms {
        if raw.latency_ms > max {
            evidence.outcome = OutcomeClass::Soft;
            evidence.error_kind = Some(ErrorKind::Slow);
            evidence.error = Some(ErrorKind::Slow.user_message(&MessageArgs {
                latency_ms: Some(raw.latency_ms),
                max_latency_ms: Some(max),
                ..MessageArgs::default()
            }));
        }
    }

    evidence
}

fn eval_assertion(assertion: &Assertion, root: &Value) -> AssertionResult {
    match resolve_path(root, &assertion.path) {
        Ok(value) => compare(assertion.op, Some(value.as_ref()), assertion.value.as_ref())
            .with_path(&assertion.path),
        Err(PathError::Missing) => {
            compare(assertion.op, None, assertion.value.as_ref()).with_path(&assertion.path)
        }
        Err(PathError::Invalid) => AssertionResult {
            path: assertion.path.clone(),
            op: assertion.op,
            ok: false,
            expected: assertion.value.clone(),
            actual: None,
            reason: Some(REASON_INVALID_PATH.into()),
        },
    }
}

fn assertion_message(fail: &AssertionResult) -> String {
    let expected = fail
        .expected
        .as_ref()
        .map(stringify_value)
        .unwrap_or_else(|| "<missing>".into());
    let actual = fail
        .actual
        .as_ref()
        .map(stringify_value)
        .unwrap_or_else(|| "<missing>".into());
    ErrorKind::Assertion.user_message(&MessageArgs {
        path: Some(&fail.path),
        expected: Some(&expected),
        actual: Some(&actual),
        ..MessageArgs::default()
    })
}

fn status_matches(expected: &ExpectedStatus, status: u16) -> bool {
    match expected {
        ExpectedStatus::TwoXx => (200..300).contains(&status),
        ExpectedStatus::Code(code) => *code == status,
        ExpectedStatus::Codes(codes) => codes.contains(&status),
    }
}

fn body_parse(mut evidence: CheckEvidence) -> CheckEvidence {
    evidence.outcome = OutcomeClass::Hard;
    evidence.error_kind = Some(ErrorKind::BodyParse);
    evidence.error = Some(ErrorKind::BodyParse.user_message(&MessageArgs::default()));
    evidence
}

fn preview(body_preview: &str) -> Option<String> {
    if body_preview.is_empty() {
        None
    } else {
        Some(body_preview.chars().take(2048).collect())
    }
}

fn nonempty(message: String) -> Option<String> {
    if message.is_empty() {
        None
    } else {
        Some(message)
    }
}
