use std::collections::HashMap;

use chrono::Utc;

use crate::domain::{CheckEvidence, HeaderSpec, HttpMethod, Service, ServiceDraft};
#[cfg(test)]
use crate::domain::{ExpectedStatus, SECRET_MASK};
use crate::eval::evaluate_at;
use crate::poller::HttpClient;
use crate::store::SecretStore;

/// In-memory check. No history write, no poller start, no `reveal_secret`.
pub async fn run_test_draft(
    secrets: &SecretStore,
    http: &HttpClient,
    draft: ServiceDraft,
) -> CheckEvidence {
    let now = Utc::now();
    match secrets.resolve_draft(&draft) {
        Ok(headers) => {
            let service = service_from_draft(&draft);
            let mut map = HashMap::new();
            for header in headers.iter() {
                if header.secret {
                    map.insert(header.key.clone(), header.value.clone());
                }
            }
            let raw = http.check(&service, &map).await;
            evaluate_at(&service, raw, now)
        }
        Err(missing) => CheckEvidence::missing_secret(&missing.key, now),
    }
}

/// Evaluator/HTTP need a `Service`. Dummy identity is never persisted.
pub fn service_from_draft(draft: &ServiceDraft) -> Service {
    let now = Utc::now();
    Service {
        id: draft.id.clone().unwrap_or_else(|| "draft".to_string()),
        name: if draft.name.is_empty() {
            "draft".into()
        } else {
            draft.name.clone()
        },
        url: draft.url.clone(),
        method: draft.method,
        headers: draft
            .headers
            .iter()
            .map(|header| HeaderSpec {
                key: header.key.clone(),
                secret: header.secret,
                value: if header.secret {
                    None
                } else {
                    Some(header.value.clone().unwrap_or_default())
                },
            })
            .collect(),
        body: if draft.method == HttpMethod::Post {
            draft.body.clone()
        } else {
            None
        },
        interval_sec: draft.interval_sec.max(crate::domain::MIN_INTERVAL_SEC),
        timeout_ms: draft.timeout_ms.clamp(500, 60_000),
        expected_status: draft.expected_status.clone(),
        assertions: draft.assertions.clone(),
        max_latency_ms: draft.max_latency_ms,
        action_url: draft.action_url.clone(),
        notify: draft.notify,
        always_alert: draft.always_alert,
        paused: false,
        follow_redirects: draft.follow_redirects.unwrap_or(true),
        fail_threshold: draft.fail_threshold,
        group: draft.group.clone(),
        created_at: now,
        updated_at: now,
    }
}

#[cfg(test)]
pub fn empty_draft_for_test() -> ServiceDraft {
    ServiceDraft {
        id: None,
        name: String::new(),
        url: String::new(),
        method: HttpMethod::Get,
        headers: Vec::new(),
        body: None,
        interval_sec: 60,
        timeout_ms: 10_000,
        expected_status: ExpectedStatus::TwoXx,
        follow_redirects: Some(true),
        assertions: Vec::new(),
        max_latency_ms: None,
        action_url: None,
        notify: true,
        always_alert: false,
        fail_threshold: None,
        group: None,
    }
}

/// Never send the UI mask as a header value (resolver already skips it).
#[cfg(test)]
pub fn draft_has_mask(draft: &ServiceDraft) -> bool {
    draft.headers.iter().any(|header| {
        header
            .value
            .as_deref()
            .is_some_and(|value| value == SECRET_MASK)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{DraftHeader, ErrorKind, OutcomeClass};
    use crate::store::SecretStore;
    use serde_json::json;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn missing_secret_returns_evidence_without_http() {
        let secrets = SecretStore::for_test();
        let http = HttpClient::new();
        let mut draft = empty_draft_for_test();
        draft.url = "https://example.invalid/health".into();
        draft.headers.push(DraftHeader {
            key: "Authorization".into(),
            value: None,
            secret: true,
            clear: false,
        });
        let evidence = run_test_draft(&secrets, &http, draft).await;
        assert_eq!(evidence.outcome, OutcomeClass::Hard);
        assert_eq!(evidence.error_kind, Some(ErrorKind::MissingSecret));
        assert_eq!(
            evidence.error.as_deref(),
            Some("Secret header Authorization is not set")
        );
        assert!(evidence.http_status.is_none());
    }

    #[tokio::test]
    async fn clear_is_missing_even_when_keychain_has_value() {
        let secrets = SecretStore::for_test();
        secrets.set("svc", "Authorization", "old-token").unwrap();
        let http = HttpClient::new();
        let mut draft = empty_draft_for_test();
        draft.id = Some("svc".into());
        draft.url = "https://example.invalid/health".into();
        draft.headers.push(DraftHeader {
            key: "Authorization".into(),
            value: None,
            secret: true,
            clear: true,
        });
        let evidence = run_test_draft(&secrets, &http, draft).await;
        assert_eq!(evidence.error_kind, Some(ErrorKind::MissingSecret));
        assert_eq!(secrets.get("svc", "Authorization").unwrap(), "old-token");
    }

    #[tokio::test]
    async fn mask_falls_back_to_keychain_and_does_not_send_mask() {
        let secrets = SecretStore::for_test();
        secrets
            .set("svc", "Authorization", "from-keychain")
            .unwrap();
        let mut draft = empty_draft_for_test();
        draft.id = Some("svc".into());
        draft.headers.push(DraftHeader {
            key: "Authorization".into(),
            value: Some(SECRET_MASK.into()),
            secret: true,
            clear: false,
        });
        let resolved = secrets.resolve_draft(&draft).unwrap();
        assert_eq!(resolved.get("Authorization"), Some("from-keychain"));
        assert_ne!(resolved.get("Authorization"), Some(SECRET_MASK));
        assert!(draft_has_mask(&draft));
    }

    #[tokio::test]
    async fn test_draft_evaluates_without_persist() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "ok"})))
            .expect(1)
            .mount(&server)
            .await;

        let secrets = SecretStore::for_test();
        let http = HttpClient::new();
        let mut draft = empty_draft_for_test();
        draft.url = format!("{}/health", server.uri());
        let evidence = run_test_draft(&secrets, &http, draft).await;
        assert_eq!(evidence.outcome, OutcomeClass::Ok);
        assert_eq!(evidence.http_status, Some(200));
        assert!(evidence.error_kind.is_none());
    }

    #[tokio::test]
    async fn test_draft_uses_draft_secret_not_reveal() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(wiremock::matchers::header(
                "Authorization",
                "Bearer draft-tok",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;

        let secrets = SecretStore::for_test();
        secrets.set("svc", "Authorization", "Bearer old").unwrap();
        let http = HttpClient::new();
        let mut draft = empty_draft_for_test();
        draft.id = Some("svc".into());
        draft.url = format!("{}/health", server.uri());
        draft.headers.push(DraftHeader {
            key: "Authorization".into(),
            value: Some("Bearer draft-tok".into()),
            secret: true,
            clear: false,
        });
        let evidence = run_test_draft(&secrets, &http, draft).await;
        assert_eq!(evidence.outcome, OutcomeClass::Ok);
        assert_eq!(secrets.get("svc", "Authorization").unwrap(), "Bearer old");
    }
}
