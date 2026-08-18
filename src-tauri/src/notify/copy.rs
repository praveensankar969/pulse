use crate::domain::{CheckEvidence, ErrorKind, MessageArgs};

/// Omit `+k more` when this many names already fit in [`DIGEST_BODY_MAX`].
const DIGEST_NAME_CAP: usize = 3;
pub const DIGEST_BODY_MAX: usize = 60;

pub fn down_title(name: &str) -> String {
    name.to_string()
}

pub fn recovered_title(name: &str) -> String {
    name.to_string()
}

pub fn recovered_body(duration_ms: u64) -> String {
    format!("Recovered · down {}", format_duration(duration_ms))
}

pub fn digest_title(n: usize) -> String {
    format!("{n} services down")
}

/// `{name1}, {name2}, +{k} more` — drop `+k` when ≤3 names fit ~60 chars.
pub fn digest_body(names: &[&str]) -> String {
    if names.is_empty() {
        return String::new();
    }
    let mut shown: Vec<&str> = Vec::new();
    let mut used = 0usize;
    for name in names {
        if shown.len() >= DIGEST_NAME_CAP {
            break;
        }
        let extra = if shown.is_empty() {
            name.len()
        } else {
            2 + name.len()
        };
        if used + extra > DIGEST_BODY_MAX && !shown.is_empty() {
            break;
        }
        shown.push(name);
        used += extra;
    }
    let remaining = names.len() - shown.len();
    let joined = shown.join(", ");
    if remaining == 0 {
        joined
    } else {
        format!("{joined}, +{remaining} more")
    }
}

/// Toast body. Never includes expected/actual, headers, or bodyPreview.
pub fn down_body(evidence: &CheckEvidence, timeout_ms: u32) -> String {
    match evidence.error_kind {
        Some(ErrorKind::UnexpectedStatus) => status_body(evidence),
        Some(ErrorKind::Timeout) => ErrorKind::Timeout.user_message(&MessageArgs {
            timeout_ms: Some(timeout_ms),
            ..MessageArgs::default()
        }),
        Some(ErrorKind::Dns) => "Couldn't resolve host".into(),
        Some(ErrorKind::TlsExpired) => "TLS: certificate expired".into(),
        Some(ErrorKind::TlsUntrusted) => "TLS: certificate untrusted".into(),
        Some(ErrorKind::TlsHostname) => "TLS: hostname mismatch".into(),
        Some(ErrorKind::TlsOther) => "TLS handshake failed".into(),
        Some(ErrorKind::Refused) => "Connection refused".into(),
        Some(ErrorKind::Reset) => "Connection reset".into(),
        Some(ErrorKind::Unreachable) => "Network unreachable".into(),
        Some(ErrorKind::TooManyRedirects) => "Too many redirects".into(),
        Some(ErrorKind::RedirectDowngrade) => "Redirect would drop HTTPS".into(),
        Some(ErrorKind::BodyParse) => with_http("Response is not JSON", evidence.http_status),
        Some(ErrorKind::Assertion) => assertion_body(evidence),
        Some(ErrorKind::InvalidUrl) => "Invalid URL".into(),
        Some(ErrorKind::MissingSecret) => evidence
            .error
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Secret header is not set".into()),
        Some(ErrorKind::Slow) => evidence
            .error
            .clone()
            .unwrap_or_else(|| "Check is slow".into()),
        Some(ErrorKind::Canceled) | Some(ErrorKind::Offline) | None => "Check failed".into(),
    }
}

fn status_body(evidence: &CheckEvidence) -> String {
    let code = evidence.http_status.unwrap_or(0);
    match evidence.latency_ms {
        Some(ms) => format!("HTTP {code} · {}", format_latency(ms)),
        None => format!("HTTP {code}"),
    }
}

fn assertion_body(evidence: &CheckEvidence) -> String {
    let fails: Vec<_> = evidence
        .assertion_results
        .iter()
        .filter(|result| !result.ok)
        .collect();
    let suffix = match evidence.http_status {
        Some(code) => format!(" · HTTP {code}"),
        None => String::new(),
    };
    match fails.as_slice() {
        [one] => format!("{} failed{suffix}", one.path),
        many if !many.is_empty() => format!("{} assertions failed{suffix}", many.len()),
        _ => format!("Assertion failed{suffix}"),
    }
}

fn with_http(prefix: &str, http_status: Option<u16>) -> String {
    match http_status {
        Some(code) => format!("{prefix} · HTTP {code}"),
        None => prefix.to_string(),
    }
}

pub fn format_latency(ms: u32) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms.is_multiple_of(1_000) {
        format!("{}s", ms / 1_000)
    } else {
        format!("{:.1}s", f64::from(ms) / 1_000.0)
    }
}

pub fn format_duration(duration_ms: u64) -> String {
    let secs = duration_ms / 1_000;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3_600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3_600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::domain::{AssertOp, AssertionResult, OutcomeClass};

    fn evidence(kind: ErrorKind, status: Option<u16>, latency: Option<u32>) -> CheckEvidence {
        CheckEvidence {
            at: Utc::now(),
            outcome: OutcomeClass::Hard,
            http_status: status,
            latency_ms: latency,
            redirects: None,
            headers_stripped_on_redirect: None,
            assertion_results: Vec::new(),
            assertion_skipped: None,
            error_kind: Some(kind),
            error: None,
            body_preview: Some("{\"token\":\"should-not-leak\"}".into()),
        }
    }

    #[test]
    fn templates_match_spec() {
        assert_eq!(
            down_body(
                &evidence(ErrorKind::UnexpectedStatus, Some(502), Some(1_400)),
                10_000
            ),
            "HTTP 502 · 1.4s"
        );
        assert_eq!(
            down_body(&evidence(ErrorKind::Timeout, None, Some(10_000)), 10_000),
            "Timed out after 10s"
        );
        assert_eq!(
            down_body(&evidence(ErrorKind::Dns, None, None), 10_000),
            "Couldn't resolve host"
        );
        assert_eq!(
            down_body(&evidence(ErrorKind::TlsExpired, None, None), 10_000),
            "TLS: certificate expired"
        );
        assert_eq!(
            down_body(&evidence(ErrorKind::TlsUntrusted, None, None), 10_000),
            "TLS: certificate untrusted"
        );
        assert_eq!(
            down_body(&evidence(ErrorKind::Refused, None, None), 10_000),
            "Connection refused"
        );
        assert_eq!(
            down_body(&evidence(ErrorKind::BodyParse, Some(204), None), 10_000),
            "Response is not JSON · HTTP 204"
        );
        assert_eq!(recovered_body(4 * 60 * 1_000), "Recovered · down 4m");
        assert_eq!(digest_title(3), "3 services down");
    }

    #[test]
    fn assertion_copy_has_no_expected_actual() {
        let mut ev = evidence(ErrorKind::Assertion, Some(200), Some(12));
        ev.assertion_results = vec![AssertionResult {
            path: "status".into(),
            op: AssertOp::Equals,
            ok: false,
            expected: Some(serde_json::json!("ok")),
            actual: Some(serde_json::json!("degraded")),
            reason: None,
        }];
        ev.error = Some("status expected ok, got degraded".into());
        ev.body_preview = Some("{\"status\":\"degraded\"}".into());
        let body = down_body(&ev, 10_000);
        assert_eq!(body, "status failed · HTTP 200");
        assert!(!body.contains("expected"));
        assert!(!body.contains("got"));
        assert!(!body.contains("degraded"));
        assert!(!body.contains("ok"));
    }

    #[test]
    fn n_assertions_failed() {
        let mut ev = evidence(ErrorKind::Assertion, Some(200), None);
        ev.assertion_results = vec![
            AssertionResult {
                path: "status".into(),
                op: AssertOp::Equals,
                ok: false,
                expected: Some(serde_json::json!("ok")),
                actual: Some(serde_json::json!("bad")),
                reason: None,
            },
            AssertionResult {
                path: "errors.length".into(),
                op: AssertOp::Equals,
                ok: false,
                expected: Some(serde_json::json!(0)),
                actual: Some(serde_json::json!(2)),
                reason: None,
            },
        ];
        assert_eq!(down_body(&ev, 10_000), "2 assertions failed · HTTP 200");
    }

    #[test]
    fn digest_omits_plus_k_when_three_names_fit() {
        assert_eq!(digest_body(&["API", "Worker"]), "API, Worker");
        assert_eq!(digest_body(&["API", "Worker", "Auth"]), "API, Worker, Auth");
        assert_eq!(
            digest_body(&["API", "Worker", "Auth", "Docs"]),
            "API, Worker, Auth, +1 more"
        );
    }

    #[test]
    fn digest_plus_k_when_names_exceed_budget() {
        let long = "Payments-API-with-a-very-long-operator-name";
        let body = digest_body(&[long, long, "Auth"]);
        assert!(body.contains("+"), "{body}");
        assert!(body.len() > 20);
    }

    #[test]
    fn format_helpers() {
        assert_eq!(format_latency(500), "500ms");
        assert_eq!(format_latency(1_000), "1s");
        assert_eq!(format_latency(1_400), "1.4s");
        assert_eq!(format_duration(12_000), "12s");
        assert_eq!(format_duration(6 * 60 * 1_000), "6m");
        assert_eq!(format_duration(2 * 3_600 * 1_000), "2h");
    }
}
