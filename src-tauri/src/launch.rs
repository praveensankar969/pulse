//! Process launch flags: first-run popover, `--paused` kill switch, Harbor `--demo`.

use std::fs;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::{
    AssertOp, Assertion, ExpectedStatus, HttpMethod, Service, DEFAULT_INTERVAL_SEC,
    DEFAULT_TIMEOUT_MS,
};
use crate::store::Paths;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaunchFlags {
    pub paused: bool,
    pub demo: bool,
}

impl LaunchFlags {
    pub fn from_args<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut flags = Self::default();
        for arg in args {
            match arg.as_ref() {
                "--paused" => flags.paused = true,
                "--demo" => flags.demo = true,
                _ => {}
            }
        }
        flags
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FirstRun {
    empty_popover_shown: bool,
}

/// One-shot: first launch shows the empty popover so the user can find the tray app.
pub fn take_first_run_popover(paths: &Paths) -> bool {
    let path = paths.first_run_file();
    if let Ok(bytes) = fs::read(&path) {
        if serde_json::from_slice::<FirstRun>(&bytes).is_ok_and(|state| state.empty_popover_shown) {
            return false;
        }
    }
    if let Ok(encoded) = serde_json::to_vec_pretty(&FirstRun {
        empty_popover_shown: true,
    }) {
        let _ = fs::write(path, encoded);
    }
    true
}

pub fn pause_all(services: &mut [Service]) {
    for service in services {
        service.paused = true;
    }
}

/// Insert Harbor fixtures that are not already present (by id or health URL).
pub fn merge_demo(mut existing: Vec<Service>, demo: Vec<Service>) -> Vec<Service> {
    for service in demo {
        if existing
            .iter()
            .any(|row| row.id == service.id || row.url == service.url)
        {
            continue;
        }
        existing.push(service);
    }
    existing
}

/// Seven Harbor rows from DESIGN.md, used by `pnpm tauri dev -- --demo`.
pub fn harbor_services(now: DateTime<Utc>) -> Vec<Service> {
    vec![
        harbor(
            "01JABCDEF0000000000000API",
            "API",
            "https://api.harbor.dev/health",
            vec![equals("status", serde_json::json!("ok"))],
            now,
            None,
        ),
        harbor(
            "01JABCDEF0000000000000WEB",
            "Web",
            "https://app.harbor.dev/api/healthz",
            vec![equals("ok", serde_json::json!(true))],
            now,
            None,
        ),
        harbor(
            "01JABCDEF0000000000000WRK",
            "Worker",
            "https://worker.harbor.dev/health",
            vec![equals("status", serde_json::json!("ok"))],
            now,
            None,
        ),
        harbor(
            "01JABCDEF0000000000000ATH",
            "Auth",
            "https://auth.harbor.dev/health",
            vec![equals("status", serde_json::json!("ok"))],
            now,
            None,
        ),
        harbor(
            "01JABCDEF0000000000000PAY",
            "Payments API",
            "https://pay.harbor.dev/health",
            vec![
                equals("status", serde_json::json!("ok")),
                equals("errors.length", serde_json::json!(0)),
            ],
            now,
            Some(HarborExtra {
                max_latency_ms: Some(800),
                action_url: Some("https://grafana.harbor.dev/d/pay".into()),
                always_alert: true,
                group: Some("prod".into()),
            }),
        ),
        harbor(
            "01JABCDEF0000000000000DOC",
            "Docs",
            "https://docs.harbor.dev/health",
            vec![equals("ok", serde_json::json!(true))],
            now,
            None,
        ),
        harbor(
            "01JABCDEF0000000000000NAS",
            "NAS",
            "https://nas.home.arpa/api/v2.0/system/info",
            vec![equals("healthy", serde_json::json!(true))],
            now,
            Some(HarborExtra {
                max_latency_ms: None,
                action_url: None,
                always_alert: false,
                group: Some("home".into()),
            }),
        ),
    ]
}

struct HarborExtra {
    max_latency_ms: Option<u32>,
    action_url: Option<String>,
    always_alert: bool,
    group: Option<String>,
}

fn equals(path: &str, value: serde_json::Value) -> Assertion {
    Assertion {
        path: path.into(),
        op: AssertOp::Equals,
        value: Some(value),
    }
}

fn harbor(
    id: &str,
    name: &str,
    url: &str,
    assertions: Vec<Assertion>,
    now: DateTime<Utc>,
    extra: Option<HarborExtra>,
) -> Service {
    let extra = extra.unwrap_or(HarborExtra {
        max_latency_ms: None,
        action_url: None,
        always_alert: false,
        group: None,
    });
    Service {
        id: id.into(),
        name: name.into(),
        url: url.into(),
        method: HttpMethod::Get,
        headers: vec![],
        body: None,
        interval_sec: DEFAULT_INTERVAL_SEC,
        timeout_ms: DEFAULT_TIMEOUT_MS,
        expected_status: ExpectedStatus::TwoXx,
        assertions,
        max_latency_ms: extra.max_latency_ms,
        action_url: extra.action_url,
        notify: true,
        always_alert: extra.always_alert,
        paused: false,
        follow_redirects: true,
        fail_threshold: None,
        group: extra.group,
        created_at: now,
        updated_at: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{ConfigStore, Paths};

    #[test]
    fn parse_paused_and_demo_flags() {
        assert_eq!(LaunchFlags::from_args(["pulse"]), LaunchFlags::default());
        assert_eq!(
            LaunchFlags::from_args(["pulse", "--paused"]),
            LaunchFlags {
                paused: true,
                demo: false
            }
        );
        assert_eq!(
            LaunchFlags::from_args(["pnpm", "tauri", "dev", "--", "--demo", "--paused"]),
            LaunchFlags {
                paused: true,
                demo: true
            }
        );
    }

    #[test]
    fn harbor_is_seven_named_health_urls() {
        let services = harbor_services(Utc::now());
        assert_eq!(services.len(), 7);
        Service::validate_list(&services).unwrap();
        let names: Vec<&str> = services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "API",
                "Web",
                "Worker",
                "Auth",
                "Payments API",
                "Docs",
                "NAS"
            ]
        );
        assert_eq!(services[0].url, "https://api.harbor.dev/health");
        assert_eq!(services[1].url, "https://app.harbor.dev/api/healthz");
        assert_eq!(services[2].url, "https://worker.harbor.dev/health");
        assert_eq!(services[3].url, "https://auth.harbor.dev/health");
        assert_eq!(services[4].url, "https://pay.harbor.dev/health");
        assert_eq!(services[5].url, "https://docs.harbor.dev/health");
        assert_eq!(
            services[6].url,
            "https://nas.home.arpa/api/v2.0/system/info"
        );
        assert_eq!(services[4].max_latency_ms, Some(800));
        assert!(services[4].always_alert);
        assert!(!services.iter().any(|s| s.paused));
    }

    #[test]
    fn merge_demo_is_idempotent_by_id_or_url() {
        let now = Utc::now();
        let demo = harbor_services(now);
        let first = merge_demo(Vec::new(), demo.clone());
        assert_eq!(first.len(), 7);
        let again = merge_demo(first.clone(), demo.clone());
        assert_eq!(again.len(), 7);
        let mut existing = first;
        existing[0].name = "Custom API".into();
        let merged = merge_demo(existing, demo);
        assert_eq!(merged.len(), 7);
        assert_eq!(merged[0].name, "Custom API");
    }

    #[test]
    fn pause_all_marks_every_service() {
        let mut services = harbor_services(Utc::now());
        pause_all(&mut services);
        assert!(services.iter().all(|s| s.paused));
    }

    #[test]
    fn first_run_popover_is_one_shot() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        ConfigStore::open(paths.clone()).unwrap();
        assert!(take_first_run_popover(&paths));
        assert!(!take_first_run_popover(&paths));
        assert!(paths.first_run_file().exists());
    }
}
