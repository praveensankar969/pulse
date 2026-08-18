use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Timeout,
    Dns,
    TlsUntrusted,
    TlsExpired,
    TlsHostname,
    TlsOther,
    Refused,
    Reset,
    Unreachable,
    TooManyRedirects,
    RedirectDowngrade,
    UnexpectedStatus,
    BodyParse,
    Assertion,
    Slow,
    Canceled,
    Offline,
    InvalidUrl,
    MissingSecret,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("intervalSec must be an integer >= {min} (got {got})")]
    IntervalTooSmall { min: u32, got: u32 },
    #[error("intervalSec must be <= {max} (got {got})")]
    IntervalTooLarge { max: u32, got: u32 },
    #[error("timeoutMs must be between 500 and 60000 (got {got})")]
    TimeoutMs { got: u32 },
    #[error("failThreshold must be between 1 and 10 (got {got})")]
    FailThreshold { got: u32 },
    #[error("maxLatencyMs must be between 1 and 60000 (got {got})")]
    MaxLatencyMs { got: u32 },
    #[error("id must not be empty")]
    Id,
    #[error("duplicate service id `{0}`")]
    DuplicateId(String),
    #[error("too many services (max 100)")]
    TooManyServices,
    #[error("name must be 1–80 characters")]
    Name,
    #[error("url must be http(s) with a host and at most 2048 characters")]
    Url,
    #[error("actionUrl must be http(s) with a host and at most 2048 characters")]
    ActionUrl,
    #[error("body is only allowed on POST")]
    BodyNotAllowed,
    #[error("body must be at most 65536 bytes")]
    BodyTooLarge,
    #[error("numeric 3xx expectedStatus requires followRedirects: false")]
    ExpectedRedirectStatus,
    #[error("expectedStatus list must not be empty")]
    ExpectedStatusEmpty,
    #[error("secret header `{0}` cannot be stored in this build")]
    SecretNotSupported(String),
    #[error("duplicate header `{0}`")]
    DuplicateHeader(String),
    #[error("header key must be 1–128 characters")]
    HeaderKey,
    #[error("header value must be at most 8192 characters")]
    HeaderValue,
    #[error("too many headers (max 32)")]
    TooManyHeaders,
    #[error("too many assertions (max 16)")]
    TooManyAssertions,
    #[error("assertion path must be 1–256 characters")]
    AssertionPath,
    #[error("assertion value exceeds 1024 bytes")]
    AssertionValue,
    #[error("group must be at most 40 characters")]
    Group,
    #[error("invalid quiet hours")]
    QuietHours,
    #[error("hotkey must be at most 64 characters")]
    Hotkey,
}
