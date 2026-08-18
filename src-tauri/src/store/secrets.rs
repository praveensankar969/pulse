use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Mutex;
#[cfg(test)]
use std::sync::Once;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::StoreError;
use crate::domain::{
    is_mask, is_mask_like, is_redacted_header, DraftHeader, Header, HeaderSpec, Service,
    ServiceDraft, SECRET_MASK,
};

pub const KEYCHAIN_SERVICE: &str = "dev.pulsebar.app";
pub const REVEAL_TTL_MS: u64 = 5_000;

/// errSecAuthFailed. Unsigned → Developer ID cannot read the old item.
const ERR_SEC_AUTH_FAILED: i32 = -25293;
/// `SecCopyErrorMessageString` for `-25293`. Display omits the numeric code.
const ERR_SEC_AUTH_FAILED_MESSAGE: &str = "the user name or passphrase you entered is not correct.";

/// Account: `{service_id}/{header_key_lower}`.
pub fn account(service_id: &str, header_key: &str) -> String {
    format!("{}/{}", service_id, header_key.to_ascii_lowercase())
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    #[error("secret header `{0}` is not set")]
    NotFound(String),
    #[error("keychain identity changed; re-enter secret header `{0}`")]
    IdentityChanged(String),
    #[error("could not store secret header `{key}` in the OS keychain")]
    Backend { key: String, message: String },
    #[error("refusing to store the UI mask as a secret")]
    MaskValue,
}

impl SecretError {
    fn backend(key: &str, err: impl ToString) -> Self {
        Self::Backend {
            key: key.to_string(),
            message: err.to_string(),
        }
    }

    pub fn key(&self) -> Option<&str> {
        match self {
            Self::NotFound(key) | Self::IdentityChanged(key) => Some(key),
            Self::Backend { key, .. } => Some(key),
            Self::MaskValue => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Secret header {key} is not set")]
pub struct MissingSecret {
    pub key: String,
    pub identity_changed: bool,
}

pub struct ResolveHeader<'a> {
    pub key: &'a str,
    pub secret: bool,
    pub value: Option<&'a str>,
    pub clear: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedHeader {
    pub key: String,
    pub value: String,
    pub secret: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedHeaders(Vec<ResolvedHeader>);

impl ResolvedHeaders {
    pub fn iter(&self) -> impl Iterator<Item = &ResolvedHeader> {
        self.0.iter()
    }

    pub fn into_vec(self) -> Vec<ResolvedHeader> {
        self.0
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|header| header.key.eq_ignore_ascii_case(key))
            .map(|header| header.value.as_str())
    }
}

impl fmt::Debug for ResolvedHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = if self.secret || is_redacted_header(&self.key) {
            SECRET_MASK
        } else {
            self.value.as_str()
        };
        f.debug_struct("ResolvedHeader")
            .field("key", &self.key)
            .field("value", &value)
            .field("secret", &self.secret)
            .finish()
    }
}

impl fmt::Debug for ResolvedHeaders {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ResolvedHeaders").field(&self.0).finish()
    }
}

pub struct SecretStore {
    service_name: String,
    entries: Mutex<HashMap<String, keyring::Entry>>,
    identity_changed: Mutex<HashSet<(String, String)>>,
}

impl SecretStore {
    pub fn new() -> Self {
        Self::with_service_name(KEYCHAIN_SERVICE)
    }

    pub fn with_service_name(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            entries: Mutex::new(HashMap::new()),
            identity_changed: Mutex::new(HashSet::new()),
        }
    }

    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    pub fn set(&self, service_id: &str, header_key: &str, value: &str) -> Result<(), SecretError> {
        if is_mask_like(value) {
            return Err(SecretError::MaskValue);
        }
        match self.with_entry(service_id, header_key, |entry| entry.set_password(value)) {
            Ok(()) => {
                self.clear_identity_flag(service_id, header_key);
                Ok(())
            }
            Err(error) => Err(classify_write(header_key, error)),
        }
    }

    pub fn get(&self, service_id: &str, header_key: &str) -> Result<String, SecretError> {
        match self.with_entry(service_id, header_key, |entry| entry.get_password()) {
            Ok(value) => Ok(value),
            Err(error) => Err(self.classify_read(service_id, header_key, error)),
        }
    }

    pub fn delete(&self, service_id: &str, header_key: &str) -> Result<(), SecretError> {
        match self.with_entry(service_id, header_key, |entry| entry.delete_credential()) {
            Ok(()) => {
                self.clear_identity_flag(service_id, header_key);
                Ok(())
            }
            Err(keyring::Error::NoEntry) => {
                self.clear_identity_flag(service_id, header_key);
                Ok(())
            }
            Err(error) => Err(classify_write(header_key, error)),
        }
    }

    pub fn has_value(&self, service_id: &str, header_key: &str) -> bool {
        self.get(service_id, header_key).is_ok()
    }

    pub fn service_identity_changed(&self, service_id: &str) -> bool {
        self.identity_changed
            .lock()
            .expect("secret identity lock")
            .iter()
            .any(|(id, _)| id == service_id)
    }

    pub fn header_for_ui(&self, service_id: &str, spec: &HeaderSpec) -> Header {
        if spec.secret {
            let has_value = self.has_value(service_id, &spec.key);
            Header {
                key: spec.key.clone(),
                value: if has_value {
                    SECRET_MASK.to_string()
                } else {
                    String::new()
                },
                secret: true,
                has_value,
            }
        } else {
            Header {
                key: spec.key.clone(),
                value: spec.value.clone().unwrap_or_default(),
                secret: false,
                has_value: spec.value.as_ref().is_some_and(|value| !value.is_empty()),
            }
        }
    }

    pub fn resolve_service(&self, service: &Service) -> Result<ResolvedHeaders, MissingSecret> {
        let headers: Vec<ResolveHeader<'_>> = service
            .headers
            .iter()
            .map(|header| ResolveHeader {
                key: &header.key,
                secret: header.secret,
                value: if header.secret {
                    None
                } else {
                    header.value.as_deref()
                },
                clear: false,
            })
            .collect();
        resolve_secrets(self, Some(&service.id), &headers)
    }

    pub fn resolve_draft(&self, draft: &ServiceDraft) -> Result<ResolvedHeaders, MissingSecret> {
        let headers: Vec<ResolveHeader<'_>> = draft
            .headers
            .iter()
            .map(|header| ResolveHeader {
                key: &header.key,
                secret: header.secret,
                value: header.value.as_deref(),
                clear: header.clear,
            })
            .collect();
        resolve_secrets(self, draft.id.as_deref(), &headers)
    }

    fn with_entry<T>(
        &self,
        service_id: &str,
        header_key: &str,
        f: impl FnOnce(&keyring::Entry) -> keyring::Result<T>,
    ) -> Result<T, keyring::Error> {
        let account = account(service_id, header_key);
        let mut entries = self.entries.lock().expect("secret store lock");
        if !entries.contains_key(&account) {
            let entry = keyring::Entry::new(&self.service_name, &account)?;
            entries.insert(account.clone(), entry);
        }
        let entry = entries.get(&account).expect("entry just inserted");
        f(entry)
    }

    fn classify_read(
        &self,
        service_id: &str,
        header_key: &str,
        error: keyring::Error,
    ) -> SecretError {
        if matches!(error, keyring::Error::NoEntry) {
            return SecretError::NotFound(header_key.to_string());
        }
        if is_identity_error(&error) {
            self.mark_identity_changed(service_id, header_key);
            return SecretError::IdentityChanged(header_key.to_string());
        }
        SecretError::backend(header_key, error)
    }

    fn mark_identity_changed(&self, service_id: &str, header_key: &str) {
        self.identity_changed
            .lock()
            .expect("secret identity lock")
            .insert((service_id.to_string(), header_key.to_ascii_lowercase()));
    }

    fn clear_identity_flag(&self, service_id: &str, header_key: &str) {
        self.identity_changed
            .lock()
            .expect("secret identity lock")
            .remove(&(service_id.to_string(), header_key.to_ascii_lowercase()));
    }
}

impl Default for SecretStore {
    fn default() -> Self {
        Self::new()
    }
}

pub fn resolve_secrets(
    store: &SecretStore,
    service_id: Option<&str>,
    headers: &[ResolveHeader<'_>],
) -> Result<ResolvedHeaders, MissingSecret> {
    let mut resolved = Vec::with_capacity(headers.len());
    for header in headers {
        if !header.secret {
            resolved.push(ResolvedHeader {
                key: header.key.to_string(),
                value: header.value.unwrap_or("").to_string(),
                secret: false,
            });
            continue;
        }
        if header.clear {
            return Err(MissingSecret {
                key: header.key.to_string(),
                identity_changed: false,
            });
        }
        if let Some(value) = header.value {
            if !is_mask_like(value) {
                resolved.push(ResolvedHeader {
                    key: header.key.to_string(),
                    value: value.to_string(),
                    secret: true,
                });
                continue;
            }
        }
        let Some(id) = service_id else {
            return Err(MissingSecret {
                key: header.key.to_string(),
                identity_changed: false,
            });
        };
        match store.get(id, header.key) {
            Ok(value) if !is_mask(&value) => resolved.push(ResolvedHeader {
                key: header.key.to_string(),
                value,
                secret: true,
            }),
            Ok(_) => {
                return Err(MissingSecret {
                    key: header.key.to_string(),
                    identity_changed: false,
                });
            }
            Err(SecretError::IdentityChanged(key)) => {
                return Err(MissingSecret {
                    key,
                    identity_changed: true,
                });
            }
            Err(SecretError::NotFound(key)) => {
                return Err(MissingSecret {
                    key,
                    identity_changed: false,
                });
            }
            Err(other) => {
                return Err(MissingSecret {
                    key: other.key().unwrap_or(header.key).to_string(),
                    identity_changed: false,
                });
            }
        }
    }
    Ok(ResolvedHeaders(resolved))
}

pub fn validate_draft_headers(draft: &[DraftHeader]) -> Result<(), StoreError> {
    let mut seen = HashSet::with_capacity(draft.len());
    for header in draft {
        if header.key.is_empty() || header.key.len() > 128 {
            return Err(crate::domain::ValidationError::HeaderKey.into());
        }
        if !seen.insert(header.key.to_ascii_lowercase()) {
            return Err(StoreError::Validation(
                crate::domain::ValidationError::DuplicateHeader(header.key.clone()),
            ));
        }
        if let Some(value) = header.value.as_deref() {
            if !is_mask(value) && value.len() > 8192 {
                return Err(crate::domain::ValidationError::HeaderValue.into());
            }
        }
    }
    Ok(())
}

pub fn persist_draft_headers(
    secrets: &SecretStore,
    service_id: &str,
    draft: &[DraftHeader],
    previous_secret_keys: &[String],
) -> Result<Vec<HeaderSpec>, StoreError> {
    validate_draft_headers(draft)?;

    let mut persisted = Vec::with_capacity(draft.len());
    for header in draft {
        if header.secret {
            if header.clear {
                secrets.delete(service_id, &header.key)?;
            } else if let Some(value) = header.value.as_deref() {
                if !is_mask_like(value) {
                    secrets.set(service_id, &header.key, value)?;
                }
            }
            persisted.push(HeaderSpec {
                key: header.key.clone(),
                secret: true,
                value: None,
            });
        } else {
            let value = header.value.clone().unwrap_or_default();
            if is_mask_like(&value) {
                return Err(SecretError::MaskValue.into());
            }
            persisted.push(HeaderSpec {
                key: header.key.clone(),
                secret: false,
                value: Some(value),
            });
        }
    }

    for old_key in previous_secret_keys {
        let still_present = draft
            .iter()
            .any(|header| header.secret && header.key.eq_ignore_ascii_case(old_key));
        if !still_present {
            secrets.delete(service_id, old_key)?;
        }
    }

    Ok(persisted)
}

pub fn delete_service_secrets(secrets: &SecretStore, service: &Service) -> Result<(), StoreError> {
    for header in service.headers.iter().filter(|header| header.secret) {
        secrets.delete(&service.id, &header.key)?;
    }
    Ok(())
}

fn classify_write(header_key: &str, error: keyring::Error) -> SecretError {
    SecretError::backend(header_key, error)
}

fn is_identity_error(error: &keyring::Error) -> bool {
    match error {
        // NoStorageAccess is a locked/missing store, not an ACL/identity miss.
        keyring::Error::PlatformFailure(inner) => is_identity_source(inner.as_ref()),
        _ => false,
    }
}

fn is_identity_source(err: &(dyn std::error::Error + Send + Sync + 'static)) -> bool {
    if identity_os_status(err) == Some(ERR_SEC_AUTH_FAILED) {
        return true;
    }
    err.to_string()
        .eq_ignore_ascii_case(ERR_SEC_AUTH_FAILED_MESSAGE)
}

fn identity_os_status(err: &(dyn std::error::Error + Send + Sync + 'static)) -> Option<i32> {
    #[cfg(target_os = "macos")]
    {
        // Must be the same security-framework major keyring boxes (3.x).
        err.downcast_ref::<security_framework::base::Error>()
            .map(|error| error.code())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = err;
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BeginRevealResponse {
    pub token: String,
    pub ttl_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RevealError {
    #[error("reveal is not available in this window")]
    ForbiddenWindow,
    #[error("invalid or expired reveal token")]
    InvalidToken,
    #[error("secret header `{0}` is not set")]
    MissingSecret(String),
    #[error("service not found")]
    NotFound,
    #[error("{0}")]
    Store(String),
}

impl serde::Serialize for RevealError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

pub fn ensure_reveal_window(label: &str) -> Result<(), RevealError> {
    if matches!(label, "detail" | "editor") {
        Ok(())
    } else {
        Err(RevealError::ForbiddenWindow)
    }
}

struct RevealSession {
    service_id: String,
    header_key: String,
    expires_at: Instant,
}

pub struct RevealRegistry {
    sessions: HashMap<String, RevealSession>,
}

impl RevealRegistry {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    pub fn begin(&mut self, service_id: &str, header_key: &str) -> BeginRevealResponse {
        self.gc();
        let token = ulid::Ulid::new().to_string();
        self.sessions.insert(
            token.clone(),
            RevealSession {
                service_id: service_id.to_string(),
                header_key: header_key.to_ascii_lowercase(),
                expires_at: Instant::now() + Duration::from_millis(REVEAL_TTL_MS),
            },
        );
        BeginRevealResponse {
            token,
            ttl_ms: REVEAL_TTL_MS,
        }
    }

    pub fn reveal(
        &mut self,
        token: &str,
        service_id: &str,
        header_key: &str,
        secrets: &SecretStore,
    ) -> Result<String, RevealError> {
        self.gc();
        let session = self.sessions.get(token).ok_or(RevealError::InvalidToken)?;
        if Instant::now() >= session.expires_at {
            self.sessions.remove(token);
            return Err(RevealError::InvalidToken);
        }
        if session.service_id != service_id || !session.header_key.eq_ignore_ascii_case(header_key)
        {
            // Bind mismatch must not burn a valid token.
            return Err(RevealError::InvalidToken);
        }
        match secrets.get(service_id, header_key) {
            Ok(value) => {
                self.sessions.remove(token);
                Ok(value)
            }
            Err(SecretError::NotFound(key) | SecretError::IdentityChanged(key)) => {
                Err(RevealError::MissingSecret(key))
            }
            Err(error) => Err(RevealError::Store(error.to_string())),
        }
    }

    pub fn end(&mut self, token: &str) {
        self.sessions.remove(token);
    }

    fn gc(&mut self) {
        let now = Instant::now();
        self.sessions.retain(|_, session| session.expires_at > now);
    }
}

impl Default for RevealRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
pub(crate) fn init_test_keyring() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    });
}

#[cfg(test)]
impl SecretStore {
    pub fn for_test() -> Self {
        init_test_keyring();
        let unique = format!(
            "dev.pulsebar.app.test.{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        );
        Self::with_service_name(unique)
    }

    pub fn set_next_error(&self, service_id: &str, header_key: &str, error: keyring::Error) {
        self.with_entry(service_id, header_key, |_| Ok(()))
            .expect("prime mock entry");
        let account = account(service_id, header_key);
        let entries = self.entries.lock().expect("secret store lock");
        let entry = entries.get(&account).expect("primed entry");
        let mock: &keyring::mock::MockCredential =
            entry.get_credential().downcast_ref().expect("keyring mock");
        mock.set_error(error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::HttpMethod;
    use chrono::Utc;

    #[derive(Debug)]
    struct DummyErr(&'static str);

    impl fmt::Display for DummyErr {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }
    }

    impl std::error::Error for DummyErr {}

    fn auth_failed_platform_error() -> keyring::Error {
        keyring::Error::PlatformFailure(Box::new(DummyErr(
            "The user name or passphrase you entered is not correct.",
        )))
    }

    fn sample_service(id: &str) -> Service {
        Service {
            id: id.to_string(),
            name: "Payments".into(),
            url: "https://pay.example/health".into(),
            method: HttpMethod::Get,
            headers: vec![
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
            ],
            body: None,
            interval_sec: 60,
            timeout_ms: 10_000,
            expected_status: crate::domain::ExpectedStatus::TwoXx,
            assertions: vec![],
            max_latency_ms: None,
            action_url: None,
            notify: true,
            always_alert: false,
            paused: false,
            follow_redirects: true,
            fail_threshold: None,
            group: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn account_lowercases_header_key() {
        assert_eq!(account("01JABC", "Authorization"), "01JABC/authorization");
    }

    #[test]
    fn per_header_roundtrip_with_mock_and_temp_service() {
        let store = SecretStore::for_test();
        assert!(store.service_name().starts_with("dev.pulsebar.app.test."));
        store.set("svc-1", "Authorization", "Bearer tok").unwrap();
        store.set("svc-1", "X-Api-Key", "abc").unwrap();
        assert_eq!(store.get("svc-1", "authorization").unwrap(), "Bearer tok");
        assert_eq!(store.get("svc-1", "X-API-KEY").unwrap(), "abc");
        assert!(matches!(
            store.get("svc-1", "Cookie"),
            Err(SecretError::NotFound(_))
        ));
    }

    #[test]
    fn refuse_to_store_ui_mask() {
        let store = SecretStore::for_test();
        assert!(matches!(
            store.set("svc", "Authorization", SECRET_MASK),
            Err(SecretError::MaskValue)
        ));
        assert!(matches!(
            store.set("svc", "Authorization", &format!("{SECRET_MASK}x")),
            Err(SecretError::MaskValue)
        ));
    }

    #[test]
    fn persist_refuses_mask_on_non_secret_header() {
        let store = SecretStore::for_test();
        let err = persist_draft_headers(
            &store,
            "svc",
            &[DraftHeader {
                key: "Authorization".into(),
                value: Some(SECRET_MASK.into()),
                secret: false,
                clear: false,
            }],
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, StoreError::Keychain { .. }));
    }

    #[test]
    fn identity_change_does_not_delete_item() {
        let store = SecretStore::for_test();
        store.set("svc", "Authorization", "keep-me").unwrap();
        store.set_next_error("svc", "Authorization", auth_failed_platform_error());
        assert!(matches!(
            store.get("svc", "Authorization"),
            Err(SecretError::IdentityChanged(_))
        ));
        assert!(store.service_identity_changed("svc"));
        assert_eq!(store.get("svc", "Authorization").unwrap(), "keep-me");
        store.set("svc", "Authorization", "new-token").unwrap();
        assert!(!store.service_identity_changed("svc"));
    }

    #[test]
    fn no_storage_access_is_not_identity_change() {
        let store = SecretStore::for_test();
        store.set("svc", "Authorization", "keep-me").unwrap();
        store.set_next_error(
            "svc",
            "Authorization",
            keyring::Error::NoStorageAccess(Box::new(DummyErr("errSecNotAvailable"))),
        );
        assert!(matches!(
            store.get("svc", "Authorization"),
            Err(SecretError::Backend { .. })
        ));
        assert!(!store.service_identity_changed("svc"));
        assert_eq!(store.get("svc", "Authorization").unwrap(), "keep-me");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn auth_failed_os_status_sets_identity_flag() {
        let store = SecretStore::for_test();
        store.set("svc", "Authorization", "keep-me").unwrap();
        store.set_next_error(
            "svc",
            "Authorization",
            keyring::Error::PlatformFailure(Box::new(security_framework::base::Error::from_code(
                ERR_SEC_AUTH_FAILED,
            ))),
        );
        assert!(matches!(
            store.get("svc", "Authorization"),
            Err(SecretError::IdentityChanged(_))
        ));
        assert!(store.service_identity_changed("svc"));
        assert_eq!(store.get("svc", "Authorization").unwrap(), "keep-me");
    }

    #[test]
    fn resolve_secrets_prefers_draft_then_keychain() {
        let store = SecretStore::for_test();
        store.set("svc", "Authorization", "from-keychain").unwrap();

        let missing = resolve_secrets(
            &store,
            Some("svc"),
            &[ResolveHeader {
                key: "Authorization",
                secret: true,
                value: None,
                clear: true,
            }],
        )
        .unwrap_err();
        assert!(!missing.identity_changed);
        assert_eq!(missing.key, "Authorization");

        let from_draft = resolve_secrets(
            &store,
            Some("svc"),
            &[ResolveHeader {
                key: "Authorization",
                secret: true,
                value: Some("from-draft"),
                clear: false,
            }],
        )
        .unwrap();
        assert_eq!(from_draft.get("Authorization"), Some("from-draft"));

        let from_mask = resolve_secrets(
            &store,
            Some("svc"),
            &[ResolveHeader {
                key: "Authorization",
                secret: true,
                value: Some(SECRET_MASK),
                clear: false,
            }],
        )
        .unwrap();
        assert_eq!(from_mask.get("Authorization"), Some("from-keychain"));
        assert_ne!(from_mask.get("Authorization"), Some(SECRET_MASK));
    }

    #[test]
    fn resolve_service_missing_secret() {
        let store = SecretStore::for_test();
        let service = sample_service("svc");
        let err = store.resolve_service(&service).unwrap_err();
        assert_eq!(err.key, "Authorization");
        assert!(!err.identity_changed);
    }

    #[test]
    fn resolve_draft_clear_skips_keychain() {
        let store = SecretStore::for_test();
        store.set("svc", "Authorization", "old").unwrap();
        let draft = ServiceDraft {
            id: Some("svc".into()),
            name: "Payments".into(),
            url: "https://pay.example/health".into(),
            method: HttpMethod::Get,
            headers: vec![DraftHeader {
                key: "Authorization".into(),
                value: None,
                secret: true,
                clear: true,
            }],
            body: None,
            interval_sec: 60,
            timeout_ms: 10_000,
            expected_status: crate::domain::ExpectedStatus::TwoXx,
            follow_redirects: Some(true),
            assertions: vec![],
            max_latency_ms: None,
            action_url: None,
            notify: true,
            always_alert: false,
            fail_threshold: None,
            group: None,
        };
        let err = store.resolve_draft(&draft).unwrap_err();
        assert_eq!(err.key, "Authorization");
        assert_eq!(store.get("svc", "Authorization").unwrap(), "old");
    }

    #[test]
    fn identity_read_failure_is_missing_secret_and_sets_flag() {
        let store = SecretStore::for_test();
        store.set("svc", "Authorization", "tok").unwrap();
        store.set_next_error("svc", "Authorization", auth_failed_platform_error());
        let service = sample_service("svc");
        let err = store.resolve_service(&service).unwrap_err();
        assert!(err.identity_changed);
        assert!(store.service_identity_changed("svc"));
        assert_eq!(store.get("svc", "Authorization").unwrap(), "tok");
    }

    #[test]
    fn store_failure_is_not_plaintext_fallback() {
        let store = SecretStore::for_test();
        store.set_next_error(
            "svc",
            "Authorization",
            keyring::Error::NoStorageAccess(Box::new(DummyErr("deny"))),
        );
        assert!(store.set("svc", "Authorization", "tok").is_err());
        assert!(matches!(
            store.get("svc", "Authorization"),
            Err(SecretError::NotFound(_))
        ));
    }

    #[test]
    fn debug_redacts_secret_and_denylist_headers() {
        let headers = ResolvedHeaders(vec![
            ResolvedHeader {
                key: "Authorization".into(),
                value: "Bearer super-secret".into(),
                secret: true,
            },
            ResolvedHeader {
                key: "Accept".into(),
                value: "application/json".into(),
                secret: false,
            },
        ]);
        let debug = format!("{headers:?}");
        assert!(!debug.contains("super-secret"));
        assert!(debug.contains(SECRET_MASK));
        assert!(debug.contains("application/json"));

        let spec = HeaderSpec {
            key: "X-Api-Key".into(),
            secret: false,
            value: Some("abc".into()),
        };
        let spec_debug = format!("{spec:?}");
        assert!(!spec_debug.contains("abc"));
        assert!(spec_debug.contains(SECRET_MASK));
    }

    #[test]
    fn persist_draft_writes_per_header_and_strips_values() {
        let store = SecretStore::for_test();
        let persisted = persist_draft_headers(
            &store,
            "svc",
            &[
                DraftHeader {
                    key: "Authorization".into(),
                    value: Some("Bearer tok".into()),
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
            &[],
        )
        .unwrap();
        assert_eq!(persisted[0].value, None);
        assert!(persisted[0].secret);
        assert_eq!(persisted[1].value.as_deref(), Some("application/json"));
        assert_eq!(store.get("svc", "Authorization").unwrap(), "Bearer tok");
    }

    #[test]
    fn persist_clear_and_removed_secret_deletes_item() {
        let store = SecretStore::for_test();
        store.set("svc", "Authorization", "old").unwrap();
        store.set("svc", "X-Api-Key", "k").unwrap();
        persist_draft_headers(
            &store,
            "svc",
            &[DraftHeader {
                key: "Authorization".into(),
                value: None,
                secret: true,
                clear: true,
            }],
            &["Authorization".into(), "X-Api-Key".into()],
        )
        .unwrap();
        assert!(matches!(
            store.get("svc", "Authorization"),
            Err(SecretError::NotFound(_))
        ));
        assert!(matches!(
            store.get("svc", "X-Api-Key"),
            Err(SecretError::NotFound(_))
        ));
    }

    #[test]
    fn persist_failure_is_hard_error() {
        let store = SecretStore::for_test();
        store.set_next_error(
            "svc",
            "Authorization",
            keyring::Error::NoStorageAccess(Box::new(DummyErr("deny write"))),
        );
        let err = persist_draft_headers(
            &store,
            "svc",
            &[DraftHeader {
                key: "Authorization".into(),
                value: Some("tok".into()),
                secret: true,
                clear: false,
            }],
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, StoreError::Keychain { .. }));
    }

    #[test]
    fn reveal_token_is_bound_and_ttl_scoped() {
        let secrets = SecretStore::for_test();
        secrets.set("svc", "Authorization", "Bearer tok").unwrap();
        let mut reveals = RevealRegistry::new();
        assert!(ensure_reveal_window("popover").is_err());
        assert!(ensure_reveal_window("detail").is_ok());
        assert!(ensure_reveal_window("editor").is_ok());

        let grant = reveals.begin("svc", "Authorization");
        assert_eq!(grant.ttl_ms, 5_000);
        assert!(reveals
            .reveal(&grant.token, "svc", "X-Api-Key", &secrets)
            .is_err());
        assert_eq!(
            reveals
                .reveal(&grant.token, "svc", "Authorization", &secrets)
                .unwrap(),
            "Bearer tok"
        );
        assert!(reveals
            .reveal(&grant.token, "svc", "Authorization", &secrets)
            .is_err());
    }

    #[test]
    fn persist_validates_before_any_keychain_write() {
        let store = SecretStore::for_test();
        store.set("svc", "Authorization", "old").unwrap();
        let err = persist_draft_headers(
            &store,
            "svc",
            &[
                DraftHeader {
                    key: "Authorization".into(),
                    value: Some("new-token".into()),
                    secret: true,
                    clear: false,
                },
                DraftHeader {
                    key: "authorization".into(),
                    value: Some("other".into()),
                    secret: true,
                    clear: false,
                },
            ],
            &[],
        )
        .unwrap_err();
        assert!(matches!(
            err,
            StoreError::Validation(crate::domain::ValidationError::DuplicateHeader(_))
        ));
        assert_eq!(store.get("svc", "Authorization").unwrap(), "old");

        let too_long = persist_draft_headers(
            &store,
            "svc-2",
            &[DraftHeader {
                key: "Authorization".into(),
                value: Some("x".repeat(8193)),
                secret: true,
                clear: false,
            }],
            &[],
        )
        .unwrap_err();
        assert!(matches!(
            too_long,
            StoreError::Validation(crate::domain::ValidationError::HeaderValue)
        ));
        assert!(matches!(
            store.get("svc-2", "Authorization"),
            Err(SecretError::NotFound(_))
        ));
    }

    #[test]
    fn ui_header_masks_existing_secret() {
        let store = SecretStore::for_test();
        store.set("svc", "Authorization", "Bearer tok").unwrap();
        let header = store.header_for_ui(
            "svc",
            &HeaderSpec {
                key: "Authorization".into(),
                secret: true,
                value: None,
            },
        );
        assert_eq!(header.value, SECRET_MASK);
        assert!(header.has_value);
        assert!(!format!("{header:?}").contains("Bearer tok"));
    }
}
