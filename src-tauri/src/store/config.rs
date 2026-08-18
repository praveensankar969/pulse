use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::migrate::{self, SCHEMA_VERSION};
use super::Paths;
use crate::domain::{AppSettings, Service, ValidationError};

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
    #[error("{0}")]
    Path(String),
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

        config.settings.validate()?;
        for service in &services.services {
            service.validate()?;
        }

        if rewrite_config {
            write_json(&paths.config_file(), &config)?;
        }
        if rewrite_services {
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
        let file = read_json::<ServicesFile>(&self.paths.services_file())?;
        migrate::ensure_supported(file.schema_version)?;
        for service in &file.services {
            service.validate()?;
        }
        Ok(file.services)
    }

    pub fn save_services(&self, services: &[Service]) -> Result<(), StoreError> {
        for service in services {
            service.validate()?;
        }
        write_json(
            &self.paths.services_file(),
            &ServicesFile {
                schema_version: SCHEMA_VERSION,
                services: services.to_vec(),
            },
        )
    }

    pub fn load_config_file(&self) -> Result<ConfigFile, StoreError> {
        let file = read_json::<ConfigFile>(&self.paths.config_file())?;
        migrate::ensure_supported(file.schema_version)?;
        file.settings.validate()?;
        Ok(file)
    }

    pub fn load_services_file(&self) -> Result<ServicesFile, StoreError> {
        let file = read_json::<ServicesFile>(&self.paths.services_file())?;
        migrate::ensure_supported(file.schema_version)?;
        for service in &file.services {
            service.validate()?;
        }
        Ok(file)
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, StoreError> {
    let bytes = fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
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
        AppSettings, HeaderSpec, Service, ValidationError, DEFAULT_FAIL_THRESHOLD,
        DEFAULT_INTERVAL_SEC, DEFAULT_TIMEOUT_MS,
    };
    use crate::store::Paths;
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
    fn secret_header_plaintext_allowed_with_feature() {
        let mut service = sample_service();
        service.headers[0].value = Some("super-secret".into());
        service.validate().unwrap();
        let (_dir, store) = open_temp();
        store.save_services(&[service.clone()]).unwrap();
        let loaded = store.load_services().unwrap();
        assert_eq!(loaded[0].headers[0].value.as_deref(), Some("super-secret"));
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
