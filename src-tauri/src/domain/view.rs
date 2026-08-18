use crate::domain::{
    CheckResult, CompactSample, Header, MachineStatus, RuntimeState, Service, ServiceStatus,
    ServiceView, SparklinePoint, UiState,
};
use crate::store::SecretStore;

pub const SPARKLINE_LEN: usize = 24;

/// Pause wins over machine status so the row reads `Paused`, not Down/Degraded.
pub fn ui_state(paused: bool, status: MachineStatus) -> UiState {
    if paused {
        return UiState::Paused;
    }
    match status {
        MachineStatus::Pending => UiState::Pending,
        MachineStatus::Healthy => UiState::Healthy,
        MachineStatus::Degraded => UiState::Degraded,
        MachineStatus::Down => UiState::Down,
    }
}

/// Last 24 post-machine samples, left-padded with `gap` (not-yet-checked / skipped).
pub fn sparkline24(samples: &[CompactSample]) -> Vec<SparklinePoint> {
    let mapped: Vec<SparklinePoint> = samples
        .iter()
        .rev()
        .take(SPARKLINE_LEN)
        .map(|sample| match sample.state {
            ServiceStatus::Healthy => SparklinePoint::Healthy,
            ServiceStatus::Degraded => SparklinePoint::Degraded,
            ServiceStatus::Down => SparklinePoint::Down,
        })
        .collect();
    let pad = SPARKLINE_LEN.saturating_sub(mapped.len());
    let mut out = vec![SparklinePoint::Gap; pad];
    out.extend(mapped.into_iter().rev());
    out
}

pub fn compact_sample(result: &CheckResult) -> CompactSample {
    CompactSample {
        at: result.evidence.at,
        state: result.state,
        outcome: result.evidence.outcome,
        http_status: result.evidence.http_status,
        latency_ms: result.evidence.latency_ms,
        error_kind: result.evidence.error_kind,
    }
}

pub fn assemble_view(
    service: &Service,
    runtime: &RuntimeState,
    last_result: Option<&CheckResult>,
    samples: &[CompactSample],
    keychain_identity_changed: bool,
    secrets: &SecretStore,
) -> ServiceView {
    let headers: Vec<Header> = service
        .headers
        .iter()
        .map(|spec| secrets.header_for_ui(&service.id, spec))
        .collect();
    ServiceView {
        service: service.clone(),
        headers,
        state: ui_state(service.paused, runtime.status),
        snooze_until: runtime.snooze_until,
        keychain_identity_changed: keychain_identity_changed.then_some(true),
        last_result: last_result.cloned(),
        last_check_at: runtime.last_check_at,
        down_since: runtime.down_since,
        degraded_since: runtime.degraded_since,
        down_clock_adjust_ms: runtime.down_clock_adjust_ms,
        consecutive_hard_fails: runtime.consecutive_hard_fails,
        sparkline24: sparkline24(samples),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        CheckEvidence, ExpectedStatus, HeaderSpec, HttpMethod, OutcomeClass, ServiceStatus,
        SECRET_MASK,
    };
    use crate::store::SecretStore;
    use chrono::{TimeZone, Utc};

    fn at() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 18, 14, 0, 0).unwrap()
    }

    fn service(paused: bool) -> Service {
        Service {
            id: "svc".into(),
            name: "Payments".into(),
            url: "https://pay.example/health".into(),
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
            paused,
            follow_redirects: true,
            fail_threshold: None,
            group: None,
            created_at: at(),
            updated_at: at(),
        }
    }

    fn sample(state: ServiceStatus) -> CompactSample {
        CompactSample {
            at: at(),
            state,
            outcome: OutcomeClass::Ok,
            http_status: Some(200),
            latency_ms: Some(12),
            error_kind: None,
        }
    }

    #[test]
    fn paused_overrides_machine_status() {
        let mut runtime = RuntimeState::pending();
        runtime.status = MachineStatus::Down;
        let view = assemble_view(
            &service(true),
            &runtime,
            None,
            &[],
            false,
            &SecretStore::for_test(),
        );
        assert_eq!(view.state, UiState::Paused);
        assert_eq!(view.consecutive_hard_fails, 0);
        assert!(view.keychain_identity_changed.is_none());
    }

    #[test]
    fn first_save_is_pending_until_applied_result() {
        let view = assemble_view(
            &service(false),
            &RuntimeState::pending(),
            None,
            &[],
            false,
            &SecretStore::for_test(),
        );
        assert_eq!(view.state, UiState::Pending);
        assert!(view.last_result.is_none());
        assert_eq!(view.sparkline24.len(), SPARKLINE_LEN);
        assert!(view
            .sparkline24
            .iter()
            .all(|point| *point == SparklinePoint::Gap));
    }

    #[test]
    fn down_clock_adjust_is_copied() {
        let mut runtime = RuntimeState::pending();
        runtime.status = MachineStatus::Down;
        runtime.down_clock_adjust_ms = 90_000;
        let view = assemble_view(
            &service(false),
            &runtime,
            None,
            &[],
            false,
            &SecretStore::for_test(),
        );
        assert_eq!(view.down_clock_adjust_ms, 90_000);
    }

    #[test]
    fn degraded_since_is_copied() {
        let mut runtime = RuntimeState::pending();
        runtime.status = MachineStatus::Degraded;
        runtime.degraded_since = Some(at());
        let view = assemble_view(
            &service(false),
            &runtime,
            None,
            &[],
            false,
            &SecretStore::for_test(),
        );
        assert_eq!(view.degraded_since, Some(at()));
    }

    #[test]
    fn identity_changed_serializes_when_set() {
        let view = assemble_view(
            &service(false),
            &RuntimeState::pending(),
            None,
            &[],
            true,
            &SecretStore::for_test(),
        );
        assert_eq!(view.keychain_identity_changed, Some(true));
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["keychainIdentityChanged"], true);
    }

    #[test]
    fn view_headers_use_has_value_not_always_mask() {
        let secrets = SecretStore::for_test();
        let mut svc = service(false);
        svc.headers = vec![
            HeaderSpec {
                key: "Authorization".into(),
                secret: true,
                value: None,
            },
            HeaderSpec {
                key: "Accept".into(),
                secret: false,
                value: Some("application/json".into()),
            },
        ];
        let empty = assemble_view(&svc, &RuntimeState::pending(), None, &[], false, &secrets);
        assert!(!empty.headers[0].has_value);
        assert_eq!(empty.headers[0].value, "");
        assert!(empty.headers[1].has_value);
        assert_eq!(empty.headers[1].value, "application/json");

        secrets.set("svc", "Authorization", "Bearer tok").unwrap();
        let filled = assemble_view(&svc, &RuntimeState::pending(), None, &[], false, &secrets);
        assert!(filled.headers[0].has_value);
        assert_eq!(filled.headers[0].value, SECRET_MASK);
        let json = serde_json::to_value(&filled).unwrap();
        assert_eq!(json["headers"][0]["hasValue"], true);
        assert_eq!(json["headers"][0]["value"], SECRET_MASK);
        assert_ne!(json["headers"][0]["value"], "Bearer tok");
    }

    #[test]
    fn sparkline_uses_last_24_and_pads_gaps() {
        let samples: Vec<_> = (0..26)
            .map(|i| {
                let state = if i == 25 {
                    ServiceStatus::Down
                } else {
                    ServiceStatus::Healthy
                };
                CompactSample {
                    at: at() + chrono::Duration::seconds(i),
                    state,
                    outcome: OutcomeClass::Ok,
                    http_status: Some(200),
                    latency_ms: Some(1),
                    error_kind: None,
                }
            })
            .collect();
        let points = sparkline24(&samples);
        assert_eq!(points.len(), 24);
        assert_eq!(points[23], SparklinePoint::Down);
        assert_eq!(points[0], SparklinePoint::Healthy);
        assert_eq!(sparkline24(&[sample(ServiceStatus::Degraded)])[23], {
            SparklinePoint::Degraded
        });
        assert_eq!(sparkline24(&[sample(ServiceStatus::Degraded)])[0], {
            SparklinePoint::Gap
        });
    }

    #[test]
    fn compact_sample_copies_post_machine_state() {
        let result = CheckResult {
            evidence: CheckEvidence {
                at: at(),
                outcome: OutcomeClass::Soft,
                http_status: Some(200),
                latency_ms: Some(900),
                redirects: None,
                headers_stripped_on_redirect: None,
                assertion_results: vec![],
                assertion_skipped: None,
                error_kind: Some(crate::domain::ErrorKind::Slow),
                error: Some("slow".into()),
                body_preview: None,
            },
            state: ServiceStatus::Degraded,
        };
        let sample = compact_sample(&result);
        assert_eq!(sample.state, ServiceStatus::Degraded);
        assert_eq!(sample.outcome, OutcomeClass::Soft);
        assert_eq!(sample.error_kind, Some(crate::domain::ErrorKind::Slow));
    }
}
