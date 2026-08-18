use std::fs;
use std::path::PathBuf;

use pulse_lib::domain::{
    AppSettings, AssertOp, AssertionSkipped, CheckEvidence, CheckResult, CompactSample, ErrorKind,
    ExpectedStatus, HttpMethod, MachineStatus, OutcomeClass, Service, ServiceStatus,
    SparklinePoint, Theme, UiState,
};
use pulse_lib::store::{ConfigFile, ServicesFile};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::json;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../schema/fixtures")
}

fn load_json(name: &str) -> serde_json::Value {
    let path = fixtures_dir().join(name);
    serde_json::from_slice(
        &fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display())),
    )
    .unwrap()
}

fn assert_roundtrip<T>(name: &str)
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let fixture = load_json(name);
    let parsed: T = serde_json::from_value(fixture.clone()).unwrap_or_else(|error| {
        panic!("{name} failed to deserialize: {error}");
    });
    let encoded = serde_json::to_value(&parsed).unwrap();
    assert_eq!(
        encoded, fixture,
        "{name}: serde JSON must match the TS fixture"
    );
    let again: T = serde_json::from_value(encoded).unwrap();
    assert_eq!(again, parsed);
}

#[test]
fn types_match() {
    assert_roundtrip::<Service>("service.json");
    assert_roundtrip::<AppSettings>("settings.json");
    assert_roundtrip::<CheckEvidence>("check-evidence.json");
    assert_roundtrip::<CheckResult>("check-result.json");
    assert_roundtrip::<ConfigFile>("config-file.json");
    assert_roundtrip::<ServicesFile>("services-file.json");

    let service: Service = serde_json::from_value(load_json("service.json")).unwrap();
    let service_json = serde_json::to_value(&service).unwrap();
    assert!(service_json.get("snoozeUntil").is_none());
    assert!(service_json.get("failThreshold").is_none());
    assert!(service_json.get("intervalSec").is_some());
    assert!(service_json.get("interval_sec").is_none());
    assert!(service.fail_threshold.is_none());
    assert_eq!(service.interval_sec, 60);
    assert_eq!(service.expected_status, ExpectedStatus::TwoXx);
    assert_eq!(service.headers[0].value, None);

    assert_eq!(
        serde_json::to_value(AppSettings::default()).unwrap(),
        load_json("settings.json")
    );

    assert_eq!(serde_json::to_value(OutcomeClass::Ok).unwrap(), json!("ok"));
    assert_eq!(
        serde_json::to_value(OutcomeClass::Soft).unwrap(),
        json!("soft")
    );
    assert_eq!(
        serde_json::to_value(OutcomeClass::Hard).unwrap(),
        json!("hard")
    );

    assert_eq!(
        serde_json::to_value(ServiceStatus::Healthy).unwrap(),
        json!("healthy")
    );
    assert_eq!(
        serde_json::to_value(ServiceStatus::Degraded).unwrap(),
        json!("degraded")
    );
    assert_eq!(
        serde_json::to_value(ServiceStatus::Down).unwrap(),
        json!("down")
    );

    assert_eq!(
        serde_json::to_value(UiState::Paused).unwrap(),
        json!("paused")
    );
    assert_eq!(
        serde_json::to_value(UiState::Pending).unwrap(),
        json!("pending")
    );
    assert_eq!(
        serde_json::to_value(MachineStatus::Pending).unwrap(),
        json!("pending")
    );

    assert_eq!(serde_json::to_value(HttpMethod::Get).unwrap(), json!("GET"));
    assert_eq!(
        serde_json::to_value(HttpMethod::Head).unwrap(),
        json!("HEAD")
    );
    assert_eq!(
        serde_json::to_value(HttpMethod::Post).unwrap(),
        json!("POST")
    );

    assert_eq!(
        serde_json::to_value(AssertOp::Equals).unwrap(),
        json!("equals")
    );
    assert_eq!(
        serde_json::to_value(AssertOp::NotEquals).unwrap(),
        json!("not_equals")
    );
    assert_eq!(
        serde_json::to_value(AssertOp::Contains).unwrap(),
        json!("contains")
    );
    assert_eq!(
        serde_json::to_value(AssertOp::Exists).unwrap(),
        json!("exists")
    );
    assert_eq!(serde_json::to_value(AssertOp::Gt).unwrap(), json!("gt"));
    assert_eq!(serde_json::to_value(AssertOp::Lt).unwrap(), json!("lt"));

    assert_eq!(
        serde_json::to_value(Theme::System).unwrap(),
        json!("system")
    );
    assert_eq!(serde_json::to_value(Theme::Dark).unwrap(), json!("dark"));
    assert_eq!(serde_json::to_value(Theme::Light).unwrap(), json!("light"));

    assert_eq!(
        serde_json::to_value(AssertionSkipped::Head).unwrap(),
        json!("head")
    );
    assert_eq!(
        serde_json::to_value(SparklinePoint::Gap).unwrap(),
        json!("gap")
    );
    assert_eq!(
        serde_json::to_value(ErrorKind::TlsUntrusted).unwrap(),
        json!("tls_untrusted")
    );
    assert_eq!(
        serde_json::to_value(ErrorKind::MissingSecret).unwrap(),
        json!("missing_secret")
    );
    assert_eq!(
        serde_json::to_value(ErrorKind::TooManyRedirects).unwrap(),
        json!("too_many_redirects")
    );

    let compact = CompactSample {
        at: service.created_at,
        state: ServiceStatus::Degraded,
        outcome: OutcomeClass::Soft,
        http_status: Some(200),
        latency_ms: Some(900),
        error_kind: Some(ErrorKind::Slow),
    };
    let compact_json = serde_json::to_value(&compact).unwrap();
    assert_eq!(compact_json["state"], "degraded");
    assert_eq!(compact_json["outcome"], "soft");
    assert_eq!(compact_json["errorKind"], "slow");
}
