use std::collections::HashSet;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{Assertion, CheckResult, SparklinePoint, UiState, ValidationError};

/// UI mask. Never persist or send this string as a header value.
pub const SECRET_MASK: &str = "••••••••";

pub fn is_mask(value: &str) -> bool {
    value == SECRET_MASK
}

pub fn is_redacted_header(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "set-cookie"
            | "x-api-key"
            | "x-auth-token"
    )
}

pub const MIN_INTERVAL_SEC: u32 = 15;
pub const MAX_INTERVAL_SEC: u32 = 600;

pub(crate) fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Head,
    Post,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedStatus {
    TwoXx,
    Code(u16),
    Codes(Vec<u16>),
}

impl Serialize for ExpectedStatus {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::TwoXx => serializer.serialize_str("2xx"),
            Self::Code(code) => serializer.serialize_u16(*code),
            Self::Codes(codes) => codes.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ExpectedStatus {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = ExpectedStatus;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("\"2xx\", an HTTP status code, or an array of status codes")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value == "2xx" {
                    Ok(ExpectedStatus::TwoXx)
                } else {
                    Err(E::invalid_value(serde::de::Unexpected::Str(value), &self))
                }
            }

            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                u16::try_from(value)
                    .ok()
                    .filter(|code| (100..=599).contains(code))
                    .map(ExpectedStatus::Code)
                    .ok_or_else(|| E::invalid_value(serde::de::Unexpected::Unsigned(value), &self))
            }

            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                u64::try_from(value)
                    .map_err(|_| E::invalid_value(serde::de::Unexpected::Signed(value), &self))
                    .and_then(|unsigned| self.visit_u64(unsigned))
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut codes = Vec::new();
                while let Some(code) = seq.next_element::<u16>()? {
                    if !(100..=599).contains(&code) {
                        return Err(A::Error::custom("status code out of range"));
                    }
                    if codes.len() >= 16 {
                        return Err(A::Error::custom("too many status codes"));
                    }
                    codes.push(code);
                }
                if codes.is_empty() {
                    return Err(A::Error::custom("expectedStatus list must not be empty"));
                }
                Ok(ExpectedStatus::Codes(codes))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderSpec {
    pub key: String,
    pub secret: bool,
    /// Plaintext only when secret == false. Secret values never sit here on disk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

impl fmt::Debug for HeaderSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HeaderSpec")
            .field("key", &self.key)
            .field("secret", &self.secret)
            .field(
                "value",
                &redacted_opt(self.secret, self.value.as_deref(), &self.key),
            )
            .finish()
    }
}

/// Wire header for the UI. Secret values are `""` or the mask, never plaintext.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    pub key: String,
    pub value: String,
    pub secret: bool,
    pub has_value: bool,
}

impl fmt::Debug for Header {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Header")
            .field("key", &self.key)
            .field(
                "value",
                &redacted_opt(self.secret, Some(self.value.as_str()), &self.key)
                    .unwrap_or(self.value.as_str()),
            )
            .field("secret", &self.secret)
            .field("has_value", &self.has_value)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftHeader {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub secret: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub clear: bool,
}

impl fmt::Debug for DraftHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DraftHeader")
            .field("key", &self.key)
            .field(
                "value",
                &redacted_opt(self.secret, self.value.as_deref(), &self.key),
            )
            .field("secret", &self.secret)
            .field("clear", &self.clear)
            .finish()
    }
}

/// Editor / Test now payload. Secret values never persist on `Service`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceDraft {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub url: String,
    pub method: HttpMethod,
    pub headers: Vec<DraftHeader>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub interval_sec: u32,
    pub timeout_ms: u32,
    pub expected_status: ExpectedStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_redirects: Option<bool>,
    pub assertions: Vec<Assertion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_latency_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_url: Option<String>,
    pub notify: bool,
    pub always_alert: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_threshold: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

fn redacted_opt<'a>(secret: bool, value: Option<&'a str>, key: &str) -> Option<&'a str> {
    match value {
        Some(_) if secret || is_redacted_header(key) => Some(SECRET_MASK),
        other => other,
    }
}

/// Persisted config. No snooze, no last result, no consecutive fails.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    pub id: String,
    pub name: String,
    pub url: String,
    pub method: HttpMethod,
    pub headers: Vec<HeaderSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub interval_sec: u32,
    pub timeout_ms: u32,
    pub expected_status: ExpectedStatus,
    pub assertions: Vec<Assertion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_latency_ms: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_url: Option<String>,
    pub notify: bool,
    pub always_alert: bool,
    pub paused: bool,
    #[serde(default = "default_true")]
    pub follow_redirects: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fail_threshold: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceView {
    #[serde(flatten)]
    pub service: Service,
    pub state: UiState,
    /// Runtime only. Never on Service / services.json / export.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snooze_until: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keychain_identity_changed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_result: Option<CheckResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_check_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub down_since: Option<DateTime<Utc>>,
    pub consecutive_hard_fails: u32,
    pub sparkline24: Vec<SparklinePoint>,
}

impl HeaderSpec {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.key.is_empty() || self.key.len() > 128 {
            return Err(ValidationError::HeaderKey);
        }
        if self.secret {
            #[cfg(not(feature = "debug-plaintext-secrets"))]
            if self.value.is_some() {
                return Err(ValidationError::SecretNotSupported(self.key.clone()));
            }
        }
        if let Some(value) = &self.value {
            if value.len() > 8192 {
                return Err(ValidationError::HeaderValue);
            }
        }
        Ok(())
    }
}

impl Service {
    pub fn validate_list(services: &[Self]) -> Result<(), ValidationError> {
        if services.len() > 100 {
            return Err(ValidationError::TooManyServices);
        }
        let mut seen = HashSet::with_capacity(services.len());
        for service in services {
            service.validate()?;
            if !seen.insert(service.id.as_str()) {
                return Err(ValidationError::DuplicateId(service.id.clone()));
            }
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.id.is_empty() {
            return Err(ValidationError::Id);
        }
        if self.name.is_empty() || self.name.chars().count() > 80 {
            return Err(ValidationError::Name);
        }
        if !valid_http_url(&self.url) {
            return Err(ValidationError::Url);
        }
        if self.interval_sec < MIN_INTERVAL_SEC {
            return Err(ValidationError::IntervalTooSmall {
                min: MIN_INTERVAL_SEC,
                got: self.interval_sec,
            });
        }
        if self.interval_sec > MAX_INTERVAL_SEC {
            return Err(ValidationError::IntervalTooLarge {
                max: MAX_INTERVAL_SEC,
                got: self.interval_sec,
            });
        }
        if !(500..=60_000).contains(&self.timeout_ms) {
            return Err(ValidationError::TimeoutMs {
                got: self.timeout_ms,
            });
        }
        if let Some(threshold) = self.fail_threshold {
            if !(1..=10).contains(&threshold) {
                return Err(ValidationError::FailThreshold { got: threshold });
            }
        }
        if let Some(max_latency_ms) = self.max_latency_ms {
            if !(1..=60_000).contains(&max_latency_ms) {
                return Err(ValidationError::MaxLatencyMs {
                    got: max_latency_ms,
                });
            }
        }
        if let Some(action_url) = &self.action_url {
            if !valid_http_url(action_url) {
                return Err(ValidationError::ActionUrl);
            }
        }
        if self.body.is_some() && self.method != HttpMethod::Post {
            return Err(ValidationError::BodyNotAllowed);
        }
        if let Some(body) = &self.body {
            if body.len() > 65_536 {
                return Err(ValidationError::BodyTooLarge);
            }
        }
        if matches!(&self.expected_status, ExpectedStatus::Codes(codes) if codes.is_empty()) {
            return Err(ValidationError::ExpectedStatusEmpty);
        }
        if self.follow_redirects && expected_has_3xx(&self.expected_status) {
            return Err(ValidationError::ExpectedRedirectStatus);
        }
        if self.headers.len() > 32 {
            return Err(ValidationError::TooManyHeaders);
        }
        for header in &self.headers {
            header.validate()?;
        }
        if self.assertions.len() > 16 {
            return Err(ValidationError::TooManyAssertions);
        }
        for assertion in &self.assertions {
            assertion.validate()?;
        }
        if let Some(group) = &self.group {
            if group.chars().count() > 40 {
                return Err(ValidationError::Group);
            }
        }
        Ok(())
    }
}

fn expected_has_3xx(status: &ExpectedStatus) -> bool {
    match status {
        ExpectedStatus::TwoXx => false,
        ExpectedStatus::Code(code) => (300..400).contains(code),
        ExpectedStatus::Codes(codes) => codes.iter().any(|code| (300..400).contains(code)),
    }
}

fn valid_http_url(value: &str) -> bool {
    if value.is_empty() || value.len() > 2048 {
        return false;
    }
    let Ok(parsed) = url::Url::parse(value) else {
        return false;
    };
    matches!(parsed.scheme(), "http" | "https") && parsed.has_host()
}
