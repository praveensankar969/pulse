use std::path::Path;
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::config::{write_json, ConfigStore};
use super::secrets::persist_draft_headers;
use super::{History, SecretStore, StoreError};
use crate::domain::{
    AppSettings, DraftHeader, ExpectedStatus, HeaderSpec, HttpMethod, QuietHours, Service, Theme,
    DEFAULT_INTERVAL_SEC, DEFAULT_TIMEOUT_MS,
};

pub const DEFAULT_EXPORT_FILENAME: &str = "pulse-services.json";
pub const SECRETS_EXPORT_FILENAME: &str = "pulse-services.SECRETS.json";

const EXPORT_SCHEMA: &str = include_str!("../../../schema/pulse-export.schema.json");

static EXPORT_VALIDATOR: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let schema: serde_json::Value =
        serde_json::from_str(EXPORT_SCHEMA).expect("pulse-export.schema.json is valid JSON");
    jsonschema::options()
        .should_validate_formats(true)
        .build(&schema)
        .expect("pulse-export.schema.json compiles")
});

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportFile {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exported_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_secrets: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<ExportSettings>,
    pub services: Vec<ExportService>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSettings {
    pub launch_at_login: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<String>,
    pub theme: Theme,
    pub default_interval: u32,
    pub default_timeout_ms: u32,
    pub fail_threshold: u32,
    pub notifications: bool,
    pub sound: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_hours: Option<QuietHours>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportService {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub method: HttpMethod,
    #[serde(default)]
    pub headers: Vec<HeaderSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default = "default_interval")]
    pub interval_sec: u32,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u32,
    #[serde(default)]
    pub expected_status: ExpectedStatus,
    #[serde(default)]
    pub assertions: Vec<crate::domain::Assertion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_latency_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_url: Option<String>,
    #[serde(default = "default_true")]
    pub notify: bool,
    #[serde(default)]
    pub always_alert: bool,
    #[serde(default)]
    pub paused: bool,
    #[serde(default = "default_true")]
    pub follow_redirects: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_threshold: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

fn default_interval() -> u32 {
    DEFAULT_INTERVAL_SEC
}

fn default_timeout() -> u32 {
    DEFAULT_TIMEOUT_MS
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    pub added: u32,
    pub updated: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPreview {
    pub filename: String,
    pub entries: Vec<(String, String)>,
    pub has_secret_values: bool,
}

pub fn export_filename(include_secrets: bool) -> &'static str {
    if include_secrets {
        SECRETS_EXPORT_FILENAME
    } else {
        DEFAULT_EXPORT_FILENAME
    }
}

pub fn confirm_message(preview: &ImportPreview, include_secrets: bool) -> String {
    let n = preview.entries.len();
    let noun = if n == 1 { "service" } else { "services" };
    let mut lines = vec![format!("Import {n} {noun} from {}?", preview.filename)];
    if !preview.entries.is_empty() {
        lines.push(String::new());
        for (name, host) in &preview.entries {
            lines.push(format!("{name} — {host}"));
        }
    }
    if include_secrets && preview.has_secret_values {
        lines.push(String::new());
        lines.push(
            "This file contains secret header values. They will be stored in your OS keychain."
                .into(),
        );
    }
    lines.join("\n")
}

pub fn parse_and_validate(bytes: &[u8]) -> Result<ExportFile, StoreError> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(StoreError::from)?;
    validate_schema(&value)?;
    Ok(serde_json::from_value(value)?)
}

fn validate_schema(value: &serde_json::Value) -> Result<(), StoreError> {
    let errors: Vec<String> = EXPORT_VALIDATOR
        .iter_errors(value)
        .take(3)
        .map(format_schema_error)
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(StoreError::InvalidExport(errors.join("\n")))
    }
}

fn format_schema_error(error: jsonschema::ValidationError<'_>) -> String {
    let path = error.instance_path.to_string();
    if path.is_empty() {
        error.to_string()
    } else {
        format!("{path}: {error}")
    }
}

pub fn has_secret_values(file: &ExportFile) -> bool {
    file.services.iter().any(|service| {
        service.headers.iter().any(|header| {
            header.secret && header.value.as_ref().is_some_and(|value| !value.is_empty())
        })
    })
}

fn reject_secrets_without_flag(file: &ExportFile, include_secrets: bool) -> Result<(), StoreError> {
    if !include_secrets && has_secret_values(file) {
        Err(StoreError::SecretsWithoutFlag)
    } else {
        Ok(())
    }
}

fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string())
}

impl ExportService {
    fn incoming_id(&self) -> Option<&str> {
        self.id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
    }

    fn to_service(&self, id: String, created_at: DateTime<Utc>, now: DateTime<Utc>) -> Service {
        Service {
            id,
            name: self.name.clone(),
            url: self.url.clone(),
            method: self.method,
            headers: self
                .headers
                .iter()
                .map(|header| HeaderSpec {
                    key: header.key.clone(),
                    secret: header.secret,
                    value: if header.secret {
                        None
                    } else {
                        header.value.clone()
                    },
                })
                .collect(),
            body: self.body.clone(),
            interval_sec: self.interval_sec,
            timeout_ms: self.timeout_ms,
            expected_status: self.expected_status.clone(),
            assertions: self.assertions.clone(),
            max_latency_ms: self.max_latency_ms,
            action_url: self.action_url.clone(),
            notify: self.notify,
            always_alert: self.always_alert,
            paused: self.paused,
            follow_redirects: self.follow_redirects,
            fail_threshold: self.fail_threshold,
            group: self.group.clone(),
            created_at,
            updated_at: now,
        }
    }

    fn draft_headers(&self) -> Vec<DraftHeader> {
        self.headers
            .iter()
            .map(|header| DraftHeader {
                key: header.key.clone(),
                value: header.value.clone(),
                secret: header.secret,
                clear: false,
            })
            .collect()
    }
}

impl ExportSettings {
    fn apply_to(&self, settings: &mut AppSettings) {
        settings.launch_at_login = self.launch_at_login;
        settings.hotkey = self.hotkey.clone();
        settings.theme = self.theme;
        settings.default_interval = self.default_interval;
        settings.default_timeout_ms = self.default_timeout_ms;
        settings.fail_threshold = self.fail_threshold;
        settings.notifications = self.notifications;
        settings.sound = self.sound;
        settings.quiet_hours = self.quiet_hours.clone();
    }
}

impl From<&AppSettings> for ExportSettings {
    fn from(settings: &AppSettings) -> Self {
        Self {
            launch_at_login: settings.launch_at_login,
            hotkey: settings.hotkey.clone(),
            theme: settings.theme,
            default_interval: settings.default_interval,
            default_timeout_ms: settings.default_timeout_ms,
            fail_threshold: settings.fail_threshold,
            notifications: settings.notifications,
            sound: settings.sound,
            quiet_hours: settings.quiet_hours.clone(),
        }
    }
}

impl ConfigStore {
    pub fn preview_import(
        bytes: &[u8],
        filename: &str,
        include_secrets: bool,
    ) -> Result<ImportPreview, StoreError> {
        let file = parse_and_validate(bytes)?;
        reject_secrets_without_flag(&file, include_secrets)?;
        let prepared = prepare_import(&[], &file)?;
        Service::validate_list(&prepared.services)?;
        Ok(ImportPreview {
            filename: filename.to_string(),
            entries: file
                .services
                .iter()
                .map(|service| (service.name.clone(), host_of(&service.url)))
                .collect(),
            has_secret_values: has_secret_values(&file),
        })
    }

    pub fn import_from_bytes(
        &self,
        secrets: &SecretStore,
        bytes: &[u8],
        include_secrets: bool,
        replace_settings: bool,
    ) -> Result<(ImportOutcome, Vec<Service>, Option<AppSettings>), StoreError> {
        let file = parse_and_validate(bytes)?;
        reject_secrets_without_flag(&file, include_secrets)?;

        let existing = self.load_services()?;
        let prepared = prepare_import(&existing, &file)?;
        Service::validate_list(&prepared.services)?;

        let mut services = prepared.services;
        for (index, drafts, previous) in prepared.secret_writes {
            let headers = persist_draft_headers(secrets, &services[index].id, &drafts, &previous)?;
            services[index].headers = headers;
        }

        self.save_services(&services)?;

        let settings = if replace_settings {
            if let Some(imported) = &file.settings {
                let mut current = self.load_settings()?;
                imported.apply_to(&mut current);
                current.validate()?;
                self.save_settings(&current)?;
                Some(current)
            } else {
                None
            }
        } else {
            None
        };

        Ok((
            ImportOutcome {
                added: prepared.added,
                updated: prepared.updated,
            },
            services,
            settings,
        ))
    }

    pub fn export_to_path(
        &self,
        secrets: &SecretStore,
        path: &Path,
        include_secrets: bool,
        include_settings: bool,
    ) -> Result<(), StoreError> {
        let file = self.build_export(secrets, include_secrets, include_settings)?;
        write_json(path, &file)?;
        let mut settings = self.load_settings()?;
        settings.last_export_at = Some(Utc::now());
        self.save_settings(&settings)?;
        Ok(())
    }

    pub fn build_export(
        &self,
        secrets: &SecretStore,
        include_secrets: bool,
        include_settings: bool,
    ) -> Result<ExportFile, StoreError> {
        let services = self.load_services()?;
        let exported = services
            .iter()
            .map(|service| export_service(service, secrets, include_secrets))
            .collect::<Result<Vec<_>, _>>()?;
        let settings = if include_settings {
            Some(ExportSettings::from(&self.load_settings()?))
        } else {
            None
        };
        Ok(ExportFile {
            schema_version: 1,
            exported_at: Some(Utc::now()),
            include_secrets: Some(include_secrets),
            settings,
            services: exported,
        })
    }

    pub fn reset_all(&self, secrets: &SecretStore, history: &History) -> Result<(), StoreError> {
        let services = self.load_services()?;
        for service in &services {
            super::secrets::delete_service_secrets(secrets, service)?;
        }
        history.clear_all()?;
        self.save_services(&[])?;
        self.save_settings(&AppSettings::default())?;
        Ok(())
    }
}

struct PreparedImport {
    services: Vec<Service>,
    secret_writes: Vec<(usize, Vec<DraftHeader>, Vec<String>)>,
    added: u32,
    updated: u32,
}

fn prepare_import(existing: &[Service], file: &ExportFile) -> Result<PreparedImport, StoreError> {
    let now = Utc::now();
    let mut services = existing.to_vec();
    let mut secret_writes = Vec::new();
    let mut added = 0;
    let mut updated = 0;

    for incoming in &file.services {
        let (index, id, created_at, previous) = match incoming.incoming_id() {
            Some(id) => {
                if let Some(index) = services.iter().position(|service| service.id == id) {
                    let existing = &services[index];
                    let previous = existing
                        .headers
                        .iter()
                        .filter(|header| header.secret)
                        .map(|header| header.key.clone())
                        .collect();
                    (
                        Some(index),
                        existing.id.clone(),
                        existing.created_at,
                        previous,
                    )
                } else {
                    (None, id.to_string(), now, Vec::new())
                }
            }
            None => (None, ulid::Ulid::new().to_string(), now, Vec::new()),
        };

        let service = incoming.to_service(id, created_at, now);
        service.validate()?;

        let drafts = incoming.draft_headers();
        match index {
            Some(index) => {
                services[index] = service;
                secret_writes.push((index, drafts, previous));
                updated += 1;
            }
            None => {
                let index = services.len();
                services.push(service);
                secret_writes.push((index, drafts, previous));
                added += 1;
            }
        }
    }

    Ok(PreparedImport {
        services,
        secret_writes,
        added,
        updated,
    })
}

fn export_service(
    service: &Service,
    secrets: &SecretStore,
    include_secrets: bool,
) -> Result<ExportService, StoreError> {
    let mut headers = Vec::with_capacity(service.headers.len());
    for header in &service.headers {
        let value = if header.secret {
            if include_secrets {
                match secrets.get(&service.id, &header.key) {
                    Ok(value) => Some(value),
                    Err(super::SecretError::NotFound(_))
                    | Err(super::SecretError::IdentityChanged(_)) => None,
                    Err(error) => return Err(error.into()),
                }
            } else {
                None
            }
        } else {
            header.value.clone()
        };
        headers.push(HeaderSpec {
            key: header.key.clone(),
            secret: header.secret,
            value,
        });
    }
    Ok(ExportService {
        id: Some(service.id.clone()),
        name: service.name.clone(),
        url: service.url.clone(),
        method: service.method,
        headers,
        body: service.body.clone(),
        interval_sec: service.interval_sec,
        timeout_ms: service.timeout_ms,
        expected_status: service.expected_status.clone(),
        assertions: service.assertions.clone(),
        max_latency_ms: service.max_latency_ms,
        action_url: service.action_url.clone(),
        notify: service.notify,
        always_alert: service.always_alert,
        paused: service.paused,
        follow_redirects: service.follow_redirects,
        fail_threshold: service.fail_threshold,
        group: service.group.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        AssertOp, Assertion, DraftHeader, RuntimeState, ServiceDraft, ValidationError, SECRET_MASK,
    };
    use crate::store::{Paths, SecretError};
    use std::fs;
    use std::path::PathBuf;

    fn import_fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/import")
            .join(name)
    }

    fn fixture_bytes(name: &str) -> Vec<u8> {
        fs::read(import_fixture(name)).unwrap()
    }

    fn schema_fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../schema/fixtures")
            .join(name)
    }

    fn sample_service() -> Service {
        serde_json::from_slice(&fs::read(schema_fixture("service.json")).unwrap()).unwrap()
    }

    fn open_temp() -> (tempfile::TempDir, ConfigStore, SecretStore, History) {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::open(Paths::new(dir.path())).unwrap();
        let secrets = SecretStore::for_test();
        let history = History::open(dir.path().join("history.sqlite3")).unwrap();
        (dir, store, secrets, history)
    }

    fn draft_from_sample(secret: &str) -> ServiceDraft {
        let service = sample_service();
        ServiceDraft {
            id: None,
            name: service.name,
            url: service.url,
            method: service.method,
            headers: vec![
                DraftHeader {
                    key: "Authorization".into(),
                    value: Some(secret.into()),
                    secret: true,
                    clear: false,
                },
                DraftHeader {
                    key: "Accept".into(),
                    value: Some("application/json".into()),
                    secret: false,
                    clear: false,
                },
            ],
            body: service.body,
            interval_sec: service.interval_sec,
            timeout_ms: service.timeout_ms,
            expected_status: service.expected_status,
            follow_redirects: Some(service.follow_redirects),
            assertions: service.assertions,
            max_latency_ms: service.max_latency_ms,
            action_url: service.action_url,
            notify: service.notify,
            always_alert: service.always_alert,
            fail_threshold: service.fail_threshold,
            group: service.group,
        }
    }

    #[test]
    fn export_filename_marks_secrets() {
        assert_eq!(export_filename(false), "pulse-services.json");
        assert_eq!(export_filename(true), "pulse-services.SECRETS.json");
    }

    #[test]
    fn file_scheme_fixture_is_rejected() {
        let err = ConfigStore::preview_import(
            &fixture_bytes("file-scheme.json"),
            "file-scheme.json",
            false,
        )
        .unwrap_err();
        assert!(matches!(err, StoreError::Validation(ValidationError::Url)));
    }

    #[test]
    fn missing_scheme_fixture_is_rejected() {
        let err = ConfigStore::preview_import(
            &fixture_bytes("missing-scheme.json"),
            "missing-scheme.json",
            false,
        )
        .unwrap_err();
        assert!(matches!(err, StoreError::Validation(ValidationError::Url)));
    }

    #[test]
    fn oversized_assertion_fixture_is_rejected() {
        let err = ConfigStore::preview_import(
            &fixture_bytes("oversized-assertion.json"),
            "oversized-assertion.json",
            false,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            StoreError::Validation(ValidationError::AssertionValue)
        ));
    }

    #[test]
    fn secrets_without_flag_fixture_is_rejected() {
        let err = ConfigStore::preview_import(
            &fixture_bytes("secrets-without-flag.json"),
            "secrets-without-flag.json",
            false,
        )
        .unwrap_err();
        assert!(matches!(err, StoreError::SecretsWithoutFlag));
        assert_eq!(
            err.to_string(),
            "This file contains secret values. Re-import with Include secrets, or strip the values."
        );
    }

    #[test]
    fn secrets_without_flag_writes_nothing() {
        let (_dir, store, secrets, _history) = open_temp();
        store
            .save_services(std::slice::from_ref(&sample_service()))
            .unwrap();
        let err = store
            .import_from_bytes(
                &secrets,
                &fixture_bytes("secrets-without-flag.json"),
                false,
                false,
            )
            .unwrap_err();
        assert!(matches!(err, StoreError::SecretsWithoutFlag));
        assert_eq!(store.load_services().unwrap().len(), 1);
        assert!(matches!(
            secrets.get("01JABCDEF0000000000000API", "Authorization"),
            Err(SecretError::NotFound(_))
        ));
    }

    #[test]
    fn schema_errors_report_first_three() {
        let bytes = br#"{
  "schemaVersion": 2,
  "services": [
    { "name": "", "url": "https://ok.example/health", "extra": true },
    { "name": "ok", "url": 1 }
  ]
}"#;
        let err = parse_and_validate(bytes).unwrap_err();
        match err {
            StoreError::InvalidExport(message) => {
                let lines: Vec<_> = message.lines().collect();
                assert!(lines.len() <= 3, "{message}");
                assert!(!lines.is_empty(), "{message}");
            }
            other => panic!("expected InvalidExport, got {other}"),
        }
    }

    #[test]
    fn confirm_lists_names_and_hosts() {
        let preview = ImportPreview {
            filename: "pulse-services.json".into(),
            entries: vec![
                ("Payments API".into(), "pay.harbor.dev".into()),
                ("NAS".into(), "nas.home.arpa".into()),
            ],
            has_secret_values: true,
        };
        let message = confirm_message(&preview, true);
        assert!(message.starts_with("Import 2 services from pulse-services.json?"));
        assert!(message.contains("Payments API — pay.harbor.dev"));
        assert!(message.contains("NAS — nas.home.arpa"));
        assert!(message.contains(
            "This file contains secret header values. They will be stored in your OS keychain."
        ));
        let quiet = confirm_message(&preview, false);
        assert!(!quiet.contains("secret header values"));
    }

    #[test]
    fn export_without_secrets_omits_values_and_settings() {
        let (_dir, store, secrets, _history) = open_temp();
        let saved = store
            .save_service(&secrets, draft_from_sample("Bearer tok"))
            .unwrap();
        let file = store.build_export(&secrets, false, false).unwrap();
        assert_eq!(file.include_secrets, Some(false));
        assert!(file.settings.is_none());
        assert_eq!(file.services[0].id.as_deref(), Some(saved.id.as_str()));
        assert!(file.services[0].headers[0].secret);
        assert_eq!(file.services[0].headers[0].value, None);
        let encoded = serde_json::to_value(&file).unwrap();
        assert!(encoded.get("createdAt").is_none());
        assert!(encoded["services"][0].get("createdAt").is_none());
        assert!(encoded["services"][0].get("updatedAt").is_none());
        assert!(encoded["services"][0].get("failThreshold").is_none());
        validate_schema(&encoded).unwrap();
    }

    #[test]
    fn export_with_secrets_includes_keychain_values() {
        let (_dir, store, secrets, _history) = open_temp();
        store
            .save_service(&secrets, draft_from_sample("Bearer tok"))
            .unwrap();
        let file = store.build_export(&secrets, true, true).unwrap();
        assert_eq!(file.include_secrets, Some(true));
        assert!(file.settings.is_some());
        assert_eq!(
            file.services[0].headers[0].value.as_deref(),
            Some("Bearer tok")
        );
        assert_ne!(
            file.services[0].headers[0].value.as_deref(),
            Some(SECRET_MASK)
        );
        let encoded = serde_json::to_value(&file).unwrap();
        validate_schema(&encoded).unwrap();
    }

    #[test]
    fn export_to_path_updates_last_export_at_only_on_success() {
        let (dir, store, secrets, _history) = open_temp();
        store
            .save_service(&secrets, draft_from_sample("Bearer tok"))
            .unwrap();
        assert!(store.load_settings().unwrap().last_export_at.is_none());
        let dest = dir.path().join("pulse-services.json");
        store.export_to_path(&secrets, &dest, false, false).unwrap();
        assert!(dest.exists());
        let raw = fs::read_to_string(&dest).unwrap();
        assert!(!raw.contains("Bearer tok"));
        assert!(store.load_settings().unwrap().last_export_at.is_some());
    }

    #[test]
    fn import_valid_fixture_adds_and_can_update() {
        let (_dir, store, secrets, _history) = open_temp();
        let (first, services, _) = store
            .import_from_bytes(&secrets, &fixture_bytes("valid.json"), false, false)
            .unwrap();
        assert_eq!(first.added, 2);
        assert_eq!(first.updated, 0);
        assert_eq!(services.len(), 2);
        assert_eq!(services[0].id, "01JABCDEF0000000000000API");
        assert_eq!(services[0].headers[0].value, None);

        let (again, services, _) = store
            .import_from_bytes(&secrets, &fixture_bytes("valid.json"), false, false)
            .unwrap();
        assert_eq!(again.added, 0);
        assert_eq!(again.updated, 2);
        assert_eq!(services.len(), 2);
    }

    #[test]
    fn import_matching_id_without_value_keeps_keychain() {
        let (_dir, store, secrets, _history) = open_temp();
        let mut draft = draft_from_sample("Bearer keep");
        draft.id = Some("01JABCDEF0000000000000API".into());
        store
            .save_services(&[Service {
                id: "01JABCDEF0000000000000API".into(),
                ..sample_service()
            }])
            .unwrap();
        store.save_service(&secrets, draft).unwrap();
        store
            .import_from_bytes(&secrets, &fixture_bytes("valid.json"), false, false)
            .unwrap();
        assert_eq!(
            secrets
                .get("01JABCDEF0000000000000API", "Authorization")
                .unwrap(),
            "Bearer keep"
        );
    }

    #[test]
    fn import_secrets_with_flag_writes_keychain() {
        let (_dir, store, secrets, _history) = open_temp();
        store
            .import_from_bytes(
                &secrets,
                &fixture_bytes("secrets-without-flag.json"),
                true,
                false,
            )
            .unwrap();
        assert_eq!(
            secrets
                .get("01JABCDEF0000000000000API", "Authorization")
                .unwrap(),
            "Bearer leaked"
        );
        let loaded = store.load_services().unwrap();
        assert_eq!(loaded[0].headers[0].value, None);
        let raw = fs::read_to_string(store.paths().services_file()).unwrap();
        assert!(!raw.contains("Bearer leaked"));
    }

    #[test]
    fn import_missing_id_gets_a_new_ulid() {
        let (_dir, store, secrets, _history) = open_temp();
        let bytes = br#"{
  "schemaVersion": 1,
  "services": [{ "name": "Anon", "url": "https://anon.example/health" }]
}"#;
        let (outcome, services, _) = store
            .import_from_bytes(&secrets, bytes, false, false)
            .unwrap();
        assert_eq!(outcome.added, 1);
        assert!(!services[0].id.is_empty());
        assert_ne!(services[0].id, "01JABCDEF0000000000000API");
    }

    #[test]
    fn import_can_replace_settings() {
        let (_dir, store, secrets, _history) = open_temp();
        let bytes = br#"{
  "schemaVersion": 1,
  "settings": {
    "launchAtLogin": true,
    "theme": "dark",
    "defaultInterval": 30,
    "defaultTimeoutMs": 5000,
    "failThreshold": 5,
    "notifications": false,
    "sound": false
  },
  "services": [{ "name": "API", "url": "https://api.example/health" }]
}"#;
        let (_, _, settings) = store
            .import_from_bytes(&secrets, bytes, false, true)
            .unwrap();
        let settings = settings.expect("settings replaced");
        assert!(settings.launch_at_login);
        assert_eq!(settings.theme, Theme::Dark);
        assert_eq!(settings.fail_threshold, 5);
        assert!(!settings.notifications);
        assert!(!settings.asked_launch_at_login);
        assert!(settings.last_export_at.is_none());
    }

    #[test]
    fn delete_service_is_transactional() {
        let (_dir, store, secrets, history) = open_temp();
        let saved = store
            .save_service(&secrets, draft_from_sample("Bearer tok"))
            .unwrap();
        history
            .put_runtime(&saved.id, &RuntimeState::pending())
            .unwrap();

        store.delete_service(&secrets, &history, &saved.id).unwrap();
        assert!(store.load_services().unwrap().is_empty());
        assert!(history.get_runtime(&saved.id).unwrap().is_none());
        assert!(matches!(
            secrets.get(&saved.id, "Authorization"),
            Err(SecretError::NotFound(_))
        ));
    }

    #[test]
    fn delete_service_keychain_failure_keeps_json_and_history() {
        let (_dir, store, secrets, history) = open_temp();
        let saved = store
            .save_service(&secrets, draft_from_sample("Bearer tok"))
            .unwrap();
        history
            .put_runtime(&saved.id, &RuntimeState::pending())
            .unwrap();
        secrets.set_next_error(
            &saved.id,
            "Authorization",
            keyring::Error::NoStorageAccess(Box::new(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "deny",
            ))),
        );
        let err = store
            .delete_service(&secrets, &history, &saved.id)
            .unwrap_err();
        assert!(matches!(err, StoreError::Keychain { .. }));
        assert_eq!(store.load_services().unwrap().len(), 1);
        assert!(history.get_runtime(&saved.id).unwrap().is_some());
        assert_eq!(
            secrets.get(&saved.id, "Authorization").unwrap(),
            "Bearer tok"
        );
    }

    #[test]
    fn import_same_id_after_delete_has_no_keychain_value() {
        let (_dir, store, secrets, history) = open_temp();
        let mut draft = draft_from_sample("Bearer tok");
        draft.id = Some("01JABCDEF0000000000000API".into());
        store
            .save_services(&[Service {
                id: "01JABCDEF0000000000000API".into(),
                ..sample_service()
            }])
            .unwrap();
        store.save_service(&secrets, draft).unwrap();
        store
            .delete_service(&secrets, &history, "01JABCDEF0000000000000API")
            .unwrap();
        store
            .import_from_bytes(&secrets, &fixture_bytes("valid.json"), false, false)
            .unwrap();
        assert!(matches!(
            secrets.get("01JABCDEF0000000000000API", "Authorization"),
            Err(SecretError::NotFound(_))
        ));
        let loaded = store.load_services().unwrap();
        assert_eq!(loaded[0].id, "01JABCDEF0000000000000API");
        assert!(loaded[0].headers[0].secret);
        assert_eq!(loaded[0].headers[0].value, None);
    }

    #[test]
    fn reset_all_wipes_json_sqlite_and_keychain() {
        let (_dir, store, secrets, history) = open_temp();
        let saved = store
            .save_service(&secrets, draft_from_sample("Bearer tok"))
            .unwrap();
        history
            .put_runtime(&saved.id, &RuntimeState::pending())
            .unwrap();
        let mut settings = store.load_settings().unwrap();
        settings.fail_threshold = 7;
        settings.theme = Theme::Dark;
        store.save_settings(&settings).unwrap();

        store.reset_all(&secrets, &history).unwrap();
        assert!(store.load_services().unwrap().is_empty());
        assert_eq!(store.load_settings().unwrap(), AppSettings::default());
        assert!(history.get_runtime(&saved.id).unwrap().is_none());
        assert!(matches!(
            secrets.get(&saved.id, "Authorization"),
            Err(SecretError::NotFound(_))
        ));
    }

    #[test]
    fn assertion_object_over_1024_bytes_is_rejected() {
        let incoming = ExportService {
            id: None,
            name: "Huge".into(),
            url: "https://example.com/health".into(),
            method: HttpMethod::Get,
            headers: vec![],
            body: None,
            interval_sec: 60,
            timeout_ms: 10_000,
            expected_status: ExpectedStatus::TwoXx,
            assertions: vec![Assertion {
                path: "data".into(),
                op: AssertOp::Equals,
                value: Some(serde_json::json!({ "pad": "x".repeat(1024) })),
            }],
            max_latency_ms: None,
            action_url: None,
            notify: true,
            always_alert: false,
            paused: false,
            follow_redirects: true,
            fail_threshold: None,
            group: None,
        };
        let err = incoming
            .to_service("id".into(), Utc::now(), Utc::now())
            .validate()
            .unwrap_err();
        assert_eq!(err, ValidationError::AssertionValue);
    }
}
