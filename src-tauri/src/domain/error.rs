use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default)]
pub struct MessageArgs<'a> {
    pub timeout_ms: Option<u32>,
    pub http_status: Option<u16>,
    pub path: Option<&'a str>,
    pub expected: Option<&'a str>,
    pub actual: Option<&'a str>,
    pub latency_ms: Option<u32>,
    pub max_latency_ms: Option<u32>,
    pub secret_key: Option<&'a str>,
}

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

impl ErrorKind {
    /// User-facing taxonomy string. Empty for `canceled` (not shown).
    pub fn user_message(self, args: &MessageArgs<'_>) -> String {
        match self {
            Self::Timeout => {
                let ms = args.timeout_ms.unwrap_or(0);
                format!("Timed out after {}s", format_timeout_s(ms))
            }
            Self::Dns => "Couldn't resolve host".into(),
            Self::TlsUntrusted => "TLS: certificate untrusted".into(),
            Self::TlsExpired => "TLS: certificate expired".into(),
            Self::TlsHostname => "TLS: hostname mismatch".into(),
            Self::TlsOther => "TLS handshake failed".into(),
            Self::Refused => "Connection refused".into(),
            Self::Reset => "Connection reset".into(),
            Self::Unreachable => "Network unreachable".into(),
            Self::TooManyRedirects => "Too many redirects".into(),
            Self::RedirectDowngrade => "Redirect would drop HTTPS".into(),
            Self::UnexpectedStatus => {
                format!("HTTP {}", args.http_status.unwrap_or(0))
            }
            Self::BodyParse => "Response is not JSON".into(),
            Self::Assertion => {
                let path = args.path.unwrap_or("");
                let expected = args.expected.unwrap_or("<missing>");
                let actual = args.actual.unwrap_or("<missing>");
                format!("{path} expected {expected}, got {actual}")
            }
            Self::Slow => {
                let latency = args.latency_ms.unwrap_or(0);
                let max = args.max_latency_ms.unwrap_or(0);
                format!("{latency}ms (limit {max}ms)")
            }
            Self::Canceled => String::new(),
            Self::Offline => "Offline".into(),
            Self::InvalidUrl => "Invalid URL".into(),
            Self::MissingSecret => {
                format!("Secret header {} is not set", args.secret_key.unwrap_or(""))
            }
        }
    }
}

fn format_timeout_s(timeout_ms: u32) -> String {
    if timeout_ms.is_multiple_of(1000) {
        (timeout_ms / 1000).to_string()
    } else {
        let secs = f64::from(timeout_ms) / 1000.0;
        format!("{secs}")
    }
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
