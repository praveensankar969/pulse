use std::fs;
use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::history::History;
use super::migrate::{self, SCHEMA_VERSION};
use super::secrets::{delete_service_secrets, persist_draft_headers, SecretError, SecretStore};
use super::Paths;
use crate::domain::{AppSettings, HeaderSpec, Service, ServiceDraft, ValidationError};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("Pulse needs to be updated to read this config.")]
    SchemaTooNew { found: u32 },
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error("failed to parse config: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to read or write config: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to read or write history: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("corrupt history: {0}")]
    Corrupt(String),
    #[error("{0}")]
    Path(String),
    #[error("service not found")]
    NotFound,
    #[error("could not store secret header `{key}` in the OS keychain: {message}")]
    Keychain { key: String, message: String },
    #[error(
        "This file contains secret values. Re-import with Include secrets, or strip the values."
    )]
    SecretsWithoutFlag,
    #[error("invalid export:\n{0}")]
    InvalidExport(String),
}

impl From<SecretError> for StoreError {
    fn from(error: SecretError) -> Self {
        match error {
            SecretError::Backend { key, message } => Self::Keychain { key, message },
            SecretError::MaskValue => Self::Keychain {
                key: String::new(),
                message: SecretError::MaskValue.to_string(),
            },
            other => Self::Keychain {
                key: other.key().unwrap_or_default().to_string(),
                message: other.to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigFile {
    pub schema_version: u32,
    pub settings: AppSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServicesFile {
    pub schema_version: u32,
    pub services: Vec<Service>,
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            settings: AppSettings::default(),
        }
    }
}

impl Default for ServicesFile {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            services: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigStore {
    paths: Paths,
}

impl ConfigStore {
    pub fn open(paths: Paths) -> Result<Self, StoreError> {
        paths.ensure_dir()?;
        if !paths.config_file().exists() {
            write_json(&paths.config_file(), &ConfigFile::default())?;
        }
        if !paths.services_file().exists() {
            write_json(&paths.services_file(), &ServicesFile::default())?;
        }

        let mut config = read_json::<ConfigFile>(&paths.config_file())?;
        let mut services = read_json::<ServicesFile>(&paths.services_file())?;

        let rewrite_config = migrate::migrate_config(&mut config)?;
        let rewrite_services = migrate::migrate_services(&mut services)?;
        let stripped = strip_secret_values(&mut services.services);

        config.settings.validate()?;
        Service::validate_list(&services.services)?;

        if rewrite_config {
            write_json(&paths.config_file(), &config)?;
        }
        if rewrite_services || stripped {
            write_json(&paths.services_file(), &services)?;
        }

        Ok(Self { paths })
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    pub fn load_settings(&self) -> Result<AppSettings, StoreError> {
        let file = read_json::<ConfigFile>(&self.paths.config_file())?;
        migrate::ensure_supported(file.schema_version)?;
        file.settings.validate()?;
        Ok(file.settings)
    }

    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), StoreError> {
        settings.validate()?;
        write_json(
            &self.paths.config_file(),
            &ConfigFile {
                schema_version: SCHEMA_VERSION,
                settings: settings.clone(),
            },
        )
    }

    pub fn load_services(&self) -> Result<Vec<Service>, StoreError> {
        Ok(self.load_services_file()?.services)
    }

    pub fn save_services(&self, services: &[Service]) -> Result<(), StoreError> {
        Service::validate_list(services)?;
        write_json(
            &self.paths.services_file(),
            &ServicesFile {
                schema_version: SCHEMA_VERSION,
                services: services.to_vec(),
            },
        )
    }

    pub fn save_service(
        &self,
        secrets: &SecretStore,
        draft: ServiceDraft,
    ) -> Result<Service, StoreError> {
        let mut services = self.load_services()?;
        let now = Utc::now();
        let (index, id, created_at, paused, previous_secret_keys) =
            if let Some(id) = draft.id.as_deref() {
                let index = services
                    .iter()
                    .position(|service| service.id == id)
                    .ok_or(StoreError::NotFound)?;
                let existing = &services[index];
                let previous = existing
                    .headers
                    .iter()
                    .filter(|header| header.secret)
                    .map(|header| header.key.clone())
                    .collect::<Vec<_>>();
                (
                    Some(index),
                    existing.id.clone(),
                    existing.created_at,
                    existing.paused,
                    previous,
                )
            } else {
                (None, ulid::Ulid::new().to_string(), now, false, Vec::new())
            };

        let preview = Service {
            id: id.clone(),
            name: draft.name,
            url: draft.url,
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
            body: draft.body,
            interval_sec: draft.interval_sec,
            timeout_ms: draft.timeout_ms,
            expected_status: draft.expected_status,
            assertions: draft.assertions,
            max_latency_ms: draft.max_latency_ms,
            action_url: draft.action_url,
            notify: draft.notify,
            always_alert: draft.always_alert,
            paused,
            follow_redirects: draft.follow_redirects.unwrap_or(true),
            fail_threshold: draft.fail_threshold,
            group: draft.group,
            created_at,
            updated_at: now,
        };
        preview.validate()?;
        super::secrets::validate_draft_headers(&draft.headers)?;

        let headers = persist_draft_headers(secrets, &id, &draft.headers, &previous_secret_keys)?;
        let service = Service { headers, ..preview };

        let created = index.is_none();
        match index {
            Some(index) => services[index] = service.clone(),
            None => services.push(service.clone()),
        }
        self.save_services(&services)?;
        if created && services.len() == 1 {
            crate::platform::autostart::notify_service_created();
        }
        Ok(service)
    }

    pub fn set_paused(&self, id: &str, paused: bool) -> Result<Service, StoreError> {
        let mut services = self.load_services()?;
        let service = services
            .iter_mut()
            .find(|service| service.id == id)
            .ok_or(StoreError::NotFound)?;
        service.paused = paused;
        service.updated_at = Utc::now();
        let out = service.clone();
        self.save_services(&services)?;
        Ok(out)
    }

    /// Secrets → history → services.json. Fail before the JSON rewrite if 2 or 3 fail.
    pub fn delete_service(
        &self,
        secrets: &SecretStore,
        history: &History,
        id: &str,
    ) -> Result<(), StoreError> {
        let mut services = self.load_services()?;
        let index = services
            .iter()
            .position(|service| service.id == id)
            .ok_or(StoreError::NotFound)?;
        let service = services[index].clone();
        delete_service_secrets(secrets, &service)?;
        history.delete_service(id)?;
        services.remove(index);
        self.save_services(&services)?;
        Ok(())
    }

    pub fn load_config_file(&self) -> Result<ConfigFile, StoreError> {
        let file = read_json::<ConfigFile>(&self.paths.config_file())?;
        migrate::ensure_supported(file.schema_version)?;
        file.settings.validate()?;
        Ok(file)
    }

    pub fn load_services_file(&self) -> Result<ServicesFile, StoreError> {
        let mut file = read_json::<ServicesFile>(&self.paths.services_file())?;
        migrate::ensure_supported(file.schema_version)?;
        if strip_secret_values(&mut file.services) {
            write_json(&self.paths.services_file(), &file)?;
        }
        Service::validate_list(&file.services)?;
        Ok(file)
    }
}

/// Secret values never sit in services.json. Strip any that leaked in.
pub fn strip_secret_values(services: &mut [Service]) -> bool {
    let mut dirty = false;
    for service in services {
        for header in &mut service.headers {
            if header.secret && header.value.is_some() {
                header.value = None;
                dirty = true;
            }
        }
    }
    dirty
}

pub(crate) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, StoreError> {
    let bytes = fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    let mut json = serde_json::to_string_pretty(value)?;
    if !json.ends_with('\n') {
        json.push('\n');
    }
    atomic_write(path, json.as_bytes())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::Path(format!("config path {} has no parent", path.display())))?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_else(|| "pulse".into())
    ));
    fs::write(&tmp, bytes)?;
    if let Err(error) = replace_file(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(tmp: &Path, dest: &Path) -> Result<(), StoreError> {
    fs::rename(tmp, dest)?;
    Ok(())
}

#[cfg(windows)]
fn replace_file(tmp: &Path, dest: &Path) -> Result<(), StoreError> {
    const ERROR_ACCESS_DENIED: i32 = 5;
    let mut delay = std::time::Duration::from_millis(2);
    for attempt in 0..8 {
        if dest.exists() {
            let _ = fs::remove_file(dest);
        }
        match fs::rename(tmp, dest) {
            Ok(()) => return Ok(()),
            Err(error) if error.raw_os_error() == Some(ERROR_ACCESS_DENIED) && attempt + 1 < 8 => {
                std::thread::sleep(delay);
                delay *= 2;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(StoreError::Io(std::io::Error::from_raw_os_error(
        ERROR_ACCESS_DENIED,
    )))
}

#[cfg(test)]
mod tests {
    use super::{atomic_write, ConfigFile, ConfigStore, ServicesFile, StoreError};
    use crate::domain::{
        AppSettings, DraftHeader, ExpectedStatus, HeaderSpec, HttpMethod, Service, ServiceDraft,
        ValidationError, DEFAULT_FAIL_THRESHOLD, DEFAULT_INTERVAL_SEC, DEFAULT_TIMEOUT_MS,
        SECRET_MASK,
    };
    use crate::store::{Paths, SecretStore};
    use std::fs;
    use std::path::PathBuf;

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../schema/fixtures")
            .join(name)
    }

    fn sample_service() -> Service {
        serde_json::from_slice(&fs::read(fixture_path("service.json")).unwrap()).unwrap()
    }

    fn open_temp() -> (tempfile::TempDir, ConfigStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::open(Paths::new(dir.path())).unwrap();
        (dir, store)
    }

    #[test]
    fn first_run_writes_defaults() {
        let (_dir, store) = open_temp();
        let settings = store.load_settings().unwrap();
        assert_eq!(settings, AppSettings::default());
        assert!(!settings.launch_at_login);
        assert_eq!(settings.default_interval, DEFAULT_INTERVAL_SEC);
        assert_eq!(settings.default_timeout_ms, DEFAULT_TIMEOUT_MS);
        assert_eq!(settings.fail_threshold, DEFAULT_FAIL_THRESHOLD);
        assert!(settings.notifications);
        assert!(settings.sound);
        assert!(store.load_services().unwrap().is_empty());

        let config: serde_json::Value =
            serde_json::from_slice(&fs::read(store.paths().config_file()).unwrap()).unwrap();
        assert_eq!(config["schemaVersion"], 1);
        assert_eq!(config["settings"]["failThreshold"], 3);
        assert!(config["settings"].get("hotkey").is_none());
        assert!(config["settings"].get("lastExportAt").is_none());
    }

    #[test]
    fn settings_and_services_roundtrip() {
        let (_dir, store) = open_temp();
        let settings = AppSettings {
            fail_threshold: 4,
            theme: crate::domain::Theme::Dark,
            ..AppSettings::default()
        };
        store.save_settings(&settings).unwrap();
        assert_eq!(store.load_settings().unwrap(), settings);

        let service = sample_service();
        store.save_services(std::slice::from_ref(&service)).unwrap();
        let loaded = store.load_services().unwrap();
        assert_eq!(loaded, vec![service]);
        assert!(loaded[0].fail_threshold.is_none());
    }

    #[test]
    fn fail_threshold_omitted_when_inheriting() {
        let service = sample_service();
        assert!(service.fail_threshold.is_none());
        let value = serde_json::to_value(&service).unwrap();
        assert!(value.get("failThreshold").is_none());
        assert!(value.get("snoozeUntil").is_none());
    }

    #[test]
    fn interval_sec_minimum_is_15() {
        let mut service = sample_service();
        service.interval_sec = 14;
        assert!(matches!(
            service.validate(),
            Err(ValidationError::IntervalTooSmall { min: 15, got: 14 })
        ));

        let (_dir, store) = open_temp();
        let err = store.save_services(&[service]).unwrap_err();
        assert!(matches!(
            err,
            StoreError::Validation(ValidationError::IntervalTooSmall { min: 15, got: 14 })
        ));
    }

    #[test]
    fn validate_rejects_persist_rules() {
        struct Case {
            name: &'static str,
            tweak: fn(&mut Service),
            expected: ValidationError,
        }
        let cases = [
            Case {
                name: "3xx with followRedirects",
                tweak: |service| {
                    service.expected_status = ExpectedStatus::Code(302);
                    service.follow_redirects = true;
                },
                expected: ValidationError::ExpectedRedirectStatus,
            },
            Case {
                name: "assertion object over 1024 bytes",
                tweak: |service| {
                    service.assertions[0].value = Some(serde_json::json!({
                        "pad": "x".repeat(1024)
                    }));
                },
                expected: ValidationError::AssertionValue,
            },
            Case {
                name: "url missing host",
                tweak: |service| service.url = "https://".into(),
                expected: ValidationError::Url,
            },
            Case {
                name: "url with embedded newline",
                tweak: |service| service.url = "http://example.com\nX-Other: 1".into(),
                expected: ValidationError::Url,
            },
            Case {
                name: "body only on POST",
                tweak: |service| {
                    service.method = HttpMethod::Get;
                    service.body = Some("{}".into());
                },
                expected: ValidationError::BodyNotAllowed,
            },
            Case {
                name: "empty id",
                tweak: |service| service.id.clear(),
                expected: ValidationError::Id,
            },
            Case {
                name: "empty expectedStatus list",
                tweak: |service| service.expected_status = ExpectedStatus::Codes(vec![]),
                expected: ValidationError::ExpectedStatusEmpty,
            },
            Case {
                name: "body too large",
                tweak: |service| {
                    service.method = HttpMethod::Post;
                    service.body = Some("x".repeat(65_537));
                },
                expected: ValidationError::BodyTooLarge,
            },
            Case {
                name: "header value too large",
                tweak: |service| {
                    service.headers[1].value = Some("x".repeat(8193));
                },
                expected: ValidationError::HeaderValue,
            },
            Case {
                name: "group too long",
                tweak: |service| service.group = Some("g".repeat(41)),
                expected: ValidationError::Group,
            },
        ];
        for case in cases {
            let mut service = sample_service();
            (case.tweak)(&mut service);
            assert_eq!(service.validate(), Err(case.expected), "{}", case.name);
        }

        let err = serde_json::from_value::<ExpectedStatus>(serde_json::json!([]))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("expectedStatus list must not be empty"),
            "{err}"
        );
    }

    #[test]
    fn save_services_rejects_duplicate_ids_and_more_than_100() {
        let (_dir, store) = open_temp();
        let first = sample_service();
        let mut second = sample_service();
        second.name = "Other".into();
        let err = store.save_services(&[first.clone(), second]).unwrap_err();
        assert!(matches!(
            err,
            StoreError::Validation(ValidationError::DuplicateId(id))
                if id == first.id
        ));

        let too_many: Vec<Service> = (0..101)
            .map(|index| {
                let mut service = sample_service();
                service.id = format!("svc-{index}");
                service
            })
            .collect();
        let err = store.save_services(&too_many).unwrap_err();
        assert!(matches!(
            err,
            StoreError::Validation(ValidationError::TooManyServices)
        ));
    }

    #[cfg(not(feature = "debug-plaintext-secrets"))]
    #[test]
    fn secret_header_with_value_is_rejected() {
        let mut service = sample_service();
        service.headers[0].value = Some("super-secret".into());
        assert!(matches!(
            service.validate(),
            Err(ValidationError::SecretNotSupported(key)) if key == "Authorization"
        ));
    }

    #[test]
    fn secret_header_without_value_is_ok() {
        let service = sample_service();
        assert_eq!(
            service.headers[0],
            HeaderSpec {
                key: "Authorization".into(),
                secret: true,
                value: None,
            }
        );
        service.validate().unwrap();
    }

    #[cfg(feature = "debug-plaintext-secrets")]
    #[test]
    fn secret_header_plaintext_is_stripped_on_load() {
        let mut service = sample_service();
        service.headers[0].value = Some("super-secret".into());
        service.validate().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        fs::create_dir_all(dir.path()).unwrap();
        write_json(
            &paths.services_file(),
            &ServicesFile {
                schema_version: 1,
                services: vec![service],
            },
        )
        .unwrap();
        write_json(&paths.config_file(), &ConfigFile::default()).unwrap();
        let store = ConfigStore::open(paths).unwrap();
        let loaded = store.load_services().unwrap();
        assert_eq!(loaded[0].headers[0].value, None);
        let raw = fs::read_to_string(store.paths().services_file()).unwrap();
        assert!(!raw.contains("super-secret"));
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
    fn save_service_puts_secrets_in_keychain_not_json() {
        let (_dir, store) = open_temp();
        let secrets = SecretStore::for_test();
        let saved = store
            .save_service(&secrets, draft_from_sample("Bearer tok"))
            .unwrap();
        assert_eq!(saved.headers[0].value, None);
        assert_eq!(
            secrets.get(&saved.id, "Authorization").unwrap(),
            "Bearer tok"
        );
        let raw = fs::read_to_string(store.paths().services_file()).unwrap();
        assert!(!raw.contains("Bearer tok"));
        let loaded = store.load_services().unwrap();
        assert_eq!(loaded[0].headers[0].value, None);
        let resolved = secrets.resolve_service(&loaded[0]).unwrap();
        assert_eq!(resolved.get("Authorization"), Some("Bearer tok"));
        assert_ne!(resolved.get("Authorization"), Some(SECRET_MASK));
    }

    #[test]
    fn save_service_keychain_failure_does_not_write_json() {
        let (_dir, store) = open_temp();
        let secrets = SecretStore::for_test();
        let mut draft = draft_from_sample("Bearer tok");
        draft.id = Some("known-id".into());
        store
            .save_services(&[Service {
                id: "known-id".into(),
                ..sample_service()
            }])
            .unwrap();
        secrets.set_next_error(
            "known-id",
            "Authorization",
            keyring::Error::NoStorageAccess(Box::new(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "deny",
            ))),
        );
        let err = store.save_service(&secrets, draft).unwrap_err();
        assert!(matches!(err, StoreError::Keychain { .. }));
        let loaded = store.load_services().unwrap();
        assert_eq!(loaded[0].headers[0].value, None);
        let raw = fs::read_to_string(store.paths().services_file()).unwrap();
        assert!(!raw.contains("Bearer tok"));
    }

    #[test]
    fn load_strips_leaked_secret_values() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        fs::create_dir_all(dir.path()).unwrap();
        let mut service = sample_service();
        service.headers[0].value = Some("leaked-secret".into());
        fs::write(
            paths.services_file(),
            serde_json::to_string_pretty(&ServicesFile {
                schema_version: 1,
                services: vec![service],
            })
            .unwrap(),
        )
        .unwrap();
        fs::write(
            paths.config_file(),
            serde_json::to_string_pretty(&ConfigFile::default()).unwrap(),
        )
        .unwrap();
        let store = ConfigStore::open(paths).unwrap();
        let loaded = store.load_services().unwrap();
        assert_eq!(loaded[0].headers[0].value, None);
        let raw = fs::read_to_string(store.paths().services_file()).unwrap();
        assert!(!raw.contains("leaked-secret"));
    }

    #[test]
    fn newer_schema_refuses_to_boot() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        fs::create_dir_all(dir.path()).unwrap();
        fs::write(
            paths.config_file(),
            r#"{
  "schemaVersion": 2,
  "settings": {}
}
"#,
        )
        .unwrap();
        fs::write(
            paths.services_file(),
            r#"{
  "schemaVersion": 1,
  "services": []
}
"#,
        )
        .unwrap();
        let err = ConfigStore::open(paths).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Pulse needs to be updated to read this config."
        );
    }

    #[test]
    fn older_schema_is_rewritten_to_v1() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        fs::create_dir_all(dir.path()).unwrap();
        fs::write(
            paths.config_file(),
            r#"{
  "schemaVersion": 0,
  "settings": {}
}
"#,
        )
        .unwrap();
        fs::write(
            paths.services_file(),
            r#"{
  "schemaVersion": 0,
  "services": []
}
"#,
        )
        .unwrap();
        let store = ConfigStore::open(paths).unwrap();
        let config = store.load_config_file().unwrap();
        let services = store.load_services_file().unwrap();
        assert_eq!(config.schema_version, 1);
        assert_eq!(services.schema_version, 1);
        let raw: serde_json::Value =
            serde_json::from_slice(&fs::read(store.paths().config_file()).unwrap()).unwrap();
        assert_eq!(raw["schemaVersion"], 1);
    }

    #[test]
    fn atomic_write_replaces_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        atomic_write(&path, b"one\n").unwrap();
        atomic_write(&path, b"two\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two\n");
        assert!(!dir.path().join(".config.json.tmp").exists());
    }

    #[test]
    fn envelopes_use_camel_case_schema_version() {
        let config = serde_json::to_value(ConfigFile::default()).unwrap();
        assert!(config.get("schemaVersion").is_some());
        assert!(config.get("schema_version").is_none());
        let services = serde_json::to_value(ServicesFile::default()).unwrap();
        assert!(services.get("schemaVersion").is_some());
        assert!(services.get("schema_version").is_none());
    }

    #[test]
    fn missing_file_is_created_without_touching_the_other() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        fs::create_dir_all(dir.path()).unwrap();
        let service = sample_service();
        let file = ServicesFile {
            schema_version: 1,
            services: vec![service.clone()],
        };
        fs::write(
            paths.services_file(),
            serde_json::to_string_pretty(&file).unwrap(),
        )
        .unwrap();
        let store = ConfigStore::open(paths).unwrap();
        assert_eq!(store.load_services().unwrap(), vec![service]);
        assert_eq!(store.load_settings().unwrap(), AppSettings::default());
    }
}
