use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, COOKIE,
    LOCATION, PROXY_AUTHORIZATION, TRANSFER_ENCODING,
};
use reqwest::{Method, StatusCode};
use url::Url;

use crate::domain::{ErrorKind, HttpMethod, Service};
use crate::eval::{RawResponse, TransportError};

pub const USER_AGENT: &str = "Pulse/1.0 (+https://github.com/praveensankar969/pulse; local health check)";
pub const MAX_BODY_BYTES: usize = 64 * 1024;
pub const BODY_PREVIEW_BYTES: usize = 2048;
pub const MAX_REDIRECTS: u8 = 3;
const MAX_CONNECT_TIMEOUT_MS: u32 = 10_000;

/// reqwest wrapper: OS trust store, system proxy + env, no cookie jar.
#[derive(Clone)]
pub struct HttpClient {
    inner: reqwest::Client,
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient {
    pub fn new() -> Self {
        Self {
            inner: build_client(Duration::from_millis(u64::from(MAX_CONNECT_TIMEOUT_MS)))
                .expect("native-tls HTTP client"),
        }
    }

    /// Secret values come from `secrets` (keychain / Test now), never from logs.
    pub async fn check(
        &self,
        service: &Service,
        secrets: &HashMap<String, String>,
    ) -> Result<RawResponse, TransportError> {
        let mut url = parse_check_url(&service.url)?;
        let (mut headers, secret_names) = build_headers(service, secrets)?;
        let mut method = match service.method {
            HttpMethod::Get => Method::GET,
            HttpMethod::Head => Method::HEAD,
            HttpMethod::Post => Method::POST,
        };
        let mut body = if service.method == HttpMethod::Post {
            service.body.clone()
        } else {
            None
        };

        let budget = Duration::from_millis(u64::from(service.timeout_ms));
        let start = Instant::now();
        let mut redirects = 0_u8;
        let mut stripped = false;

        loop {
            let remaining = remaining_budget(start, budget)
                .ok_or_else(|| transport(ErrorKind::Timeout, Some(elapsed_ms(start))))?;

            let prepared = PreparedRequest {
                method: method.clone(),
                url: url.clone(),
                headers: &headers,
                secret_names: &secret_names,
            };
            let mut request = self
                .inner
                .request(prepared.method.clone(), prepared.url.clone())
                .headers(prepared.headers.clone())
                .timeout(remaining);
            if let Some(payload) = &body {
                request = request.body(payload.clone());
            }

            let response = match request.send().await {
                Ok(response) => response,
                Err(err) => return Err(map_reqwest_error(err, start)),
            };

            let status = response.status();
            if !status.is_redirection() || !service.follow_redirects {
                return read_body(response, redirects, stripped, start).await;
            }

            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            drop(response);

            match decide_redirect(&url, location.as_deref(), redirects) {
                RedirectDecision::Follow { next, strip } => {
                    if strip {
                        stripped |= strip_sensitive(&mut headers, &secret_names);
                    }
                    rewrite_method_for_redirect(status, &mut method, &mut body, &mut headers);
                    url = next;
                    redirects += 1;
                }
                RedirectDecision::TooMany => {
                    return Err(transport(
                        ErrorKind::TooManyRedirects,
                        Some(elapsed_ms(start)),
                    ));
                }
                RedirectDecision::Downgrade => {
                    return Err(transport(
                        ErrorKind::RedirectDowngrade,
                        Some(elapsed_ms(start)),
                    ));
                }
                RedirectDecision::InvalidTarget => {
                    return Err(transport(ErrorKind::InvalidUrl, Some(elapsed_ms(start))));
                }
                RedirectDecision::NotRedirect => {
                    return empty_redirect_response(status, redirects, stripped, start);
                }
            }
        }
    }
}

fn build_client(connect_timeout: Duration) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(connect_timeout)
        // reqwest only strips Authorization/Cookie; we follow hops ourselves.
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .https_only(false)
        .tls_built_in_root_certs(true)
        // OS getaddrinfo (no hickory). reqwest 0.12 has no knob for
        // hyper-util's 300ms Happy Eyeballs connect race.
        .no_hickory_dns()
        .build()
}

fn parse_check_url(raw: &str) -> Result<Url, TransportError> {
    let url = Url::parse(raw).map_err(|_| transport(ErrorKind::InvalidUrl, None))?;
    if !matches!(url.scheme(), "http" | "https") || !url.has_host() {
        return Err(transport(ErrorKind::InvalidUrl, None));
    }
    Ok(url)
}

fn build_headers(
    service: &Service,
    secrets: &HashMap<String, String>,
) -> Result<(HeaderMap, Vec<HeaderName>), TransportError> {
    let mut headers = HeaderMap::new();
    let mut secret_names = Vec::new();
    for spec in &service.headers {
        let name = HeaderName::from_bytes(spec.key.as_bytes())
            .map_err(|_| transport(ErrorKind::InvalidUrl, None))?;
        let value = if spec.secret {
            match lookup_secret(secrets, &spec.key)
                .map(str::to_owned)
                .or_else(|| spec.value.clone().filter(|v| !v.is_empty()))
            {
                Some(value) => value,
                None => {
                    return Err(TransportError {
                        kind: ErrorKind::MissingSecret,
                        latency_ms: None,
                        detail: Some(spec.key.clone()),
                    });
                }
            }
        } else {
            spec.value.clone().unwrap_or_default()
        };
        let sensitive = spec.secret || is_redacted_name(&name);
        let mut header_value =
            HeaderValue::from_str(&value).map_err(|_| transport(ErrorKind::InvalidUrl, None))?;
        if sensitive {
            header_value.set_sensitive(true);
        }
        if spec.secret {
            secret_names.push(name.clone());
        }
        headers.insert(name, header_value);
    }
    Ok((headers, secret_names))
}

fn lookup_secret<'a>(secrets: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    secrets
        .get(key)
        .or_else(|| {
            secrets
                .iter()
                .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
                .map(|(_, value)| value)
        })
        .map(String::as_str)
        .filter(|value| !value.is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RedirectDecision {
    Follow { next: Url, strip: bool },
    TooMany,
    Downgrade,
    InvalidTarget,
    NotRedirect,
}

fn decide_redirect(from: &Url, location: Option<&str>, hops: u8) -> RedirectDecision {
    if hops >= MAX_REDIRECTS {
        return RedirectDecision::TooMany;
    }
    let Some(location) = location else {
        return RedirectDecision::NotRedirect;
    };
    let Ok(next) = from.join(location) else {
        return RedirectDecision::InvalidTarget;
    };
    if !matches!(next.scheme(), "http" | "https") || !next.has_host() {
        return RedirectDecision::InvalidTarget;
    }
    if from.scheme() == "https" && next.scheme() == "http" {
        return RedirectDecision::Downgrade;
    }
    let strip = !same_host(from, &next) || from.scheme() != next.scheme();
    RedirectDecision::Follow { next, strip }
}

fn same_host(left: &Url, right: &Url) -> bool {
    match (left.host_str(), right.host_str()) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => false,
    }
}

fn strip_sensitive(headers: &mut HeaderMap, secret_names: &[HeaderName]) -> bool {
    let mut stripped = false;
    for name in [
        AUTHORIZATION,
        PROXY_AUTHORIZATION,
        COOKIE,
        HeaderName::from_static("x-api-key"),
        HeaderName::from_static("x-auth-token"),
    ] {
        if headers.remove(&name).is_some() {
            stripped = true;
        }
    }
    for name in secret_names {
        if headers.remove(name).is_some() {
            stripped = true;
        }
    }
    stripped
}

fn rewrite_method_for_redirect(
    status: StatusCode,
    method: &mut Method,
    body: &mut Option<String>,
    headers: &mut HeaderMap,
) {
    if matches!(status.as_u16(), 301..=303) && *method != Method::GET && *method != Method::HEAD {
        *method = Method::GET;
        *body = None;
        headers.remove(CONTENT_LENGTH);
        headers.remove(CONTENT_TYPE);
        headers.remove(TRANSFER_ENCODING);
    }
}

async fn read_body(
    mut response: reqwest::Response,
    redirects: u8,
    stripped: bool,
    start: Instant,
) -> Result<RawResponse, TransportError> {
    let status = response.status().as_u16();
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body.len() >= MAX_BODY_BYTES {
                    break;
                }
                let take = MAX_BODY_BYTES - body.len();
                body.extend_from_slice(&chunk[..take.min(chunk.len())]);
            }
            Ok(None) => break,
            Err(err) => return Err(map_reqwest_error(err, start)),
        }
    }
    Ok(RawResponse {
        status,
        latency_ms: elapsed_ms(start),
        redirects,
        headers_stripped_on_redirect: stripped,
        body_preview: preview_lossy(&body),
        body,
    })
}

fn empty_redirect_response(
    status: StatusCode,
    redirects: u8,
    stripped: bool,
    start: Instant,
) -> Result<RawResponse, TransportError> {
    Ok(RawResponse {
        status: status.as_u16(),
        latency_ms: elapsed_ms(start),
        redirects,
        headers_stripped_on_redirect: stripped,
        body: Vec::new(),
        body_preview: String::new(),
    })
}

fn preview_lossy(body: &[u8]) -> String {
    String::from_utf8_lossy(&body[..body.len().min(BODY_PREVIEW_BYTES)]).into_owned()
}

fn remaining_budget(start: Instant, budget: Duration) -> Option<Duration> {
    budget
        .checked_sub(start.elapsed())
        .filter(|left| !left.is_zero())
}

fn elapsed_ms(start: Instant) -> u32 {
    u32::try_from(start.elapsed().as_millis()).unwrap_or(u32::MAX)
}

fn transport(kind: ErrorKind, latency_ms: Option<u32>) -> TransportError {
    TransportError {
        kind,
        latency_ms,
        detail: None,
    }
}

fn map_reqwest_error(err: reqwest::Error, start: Instant) -> TransportError {
    transport(classify_reqwest_error(&err), Some(elapsed_ms(start)))
}

fn classify_reqwest_error(err: &reqwest::Error) -> ErrorKind {
    if err.is_timeout() {
        return ErrorKind::Timeout;
    }
    if err.is_builder() {
        return ErrorKind::InvalidUrl;
    }
    let mut saw_tls = false;
    let mut current: Option<&dyn std::error::Error> = Some(err);
    while let Some(cause) = current {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            if let Some(kind) = classify_message(&io.to_string()) {
                return kind;
            }
            match io.kind() {
                std::io::ErrorKind::ConnectionRefused => return ErrorKind::Refused,
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe => {
                    return ErrorKind::Reset;
                }
                std::io::ErrorKind::TimedOut => return ErrorKind::Timeout,
                std::io::ErrorKind::HostUnreachable
                | std::io::ErrorKind::NetworkUnreachable
                | std::io::ErrorKind::AddrNotAvailable => {
                    return ErrorKind::Unreachable;
                }
                _ => {}
            }
        }
        if let Some(kind) = classify_message(&cause.to_string()) {
            return kind;
        }
        let lower = cause.to_string().to_ascii_lowercase();
        if lower.contains("tls") || lower.contains("ssl") || lower.contains("certificate") {
            saw_tls = true;
        }
        current = cause.source();
    }
    if saw_tls {
        return ErrorKind::TlsOther;
    }
    if err.is_connect() {
        return ErrorKind::Unreachable;
    }
    ErrorKind::Unreachable
}

fn classify_message(msg: &str) -> Option<ErrorKind> {
    let m = msg.to_ascii_lowercase();
    if m.contains("timed out") || m.contains("timeout") {
        return Some(ErrorKind::Timeout);
    }
    if m.contains("nodename nor servname")
        || m.contains("failed to lookup")
        || m.contains("no such host")
        || m.contains("name or service not known")
        || m.contains("temporary failure in name resolution")
        || m.contains("no address associated")
    {
        return Some(ErrorKind::Dns);
    }
    if m.contains("certificate has expired")
        || m.contains("certificate expired")
        || m.contains("cert_e_expired")
        || m.contains("errsslcertexpired")
    {
        return Some(ErrorKind::TlsExpired);
    }
    if m.contains("hostname mismatch")
        || m.contains("cert_e_cn_no_match")
        || m.contains("errsslhostnamemismatch")
        || (m.contains("does not match") && m.contains("host"))
        || m.contains("certificate not valid for")
    {
        return Some(ErrorKind::TlsHostname);
    }
    if m.contains("unknown issuer")
        || m.contains("unknown ca")
        || m.contains("untrusted")
        || m.contains("self signed")
        || m.contains("self-signed")
        || m.contains("cert_e_untrustedroot")
        || m.contains("not trusted")
        || m.contains("certificate verify failed")
        || m.contains("invalid certificate")
    {
        return Some(ErrorKind::TlsUntrusted);
    }
    if m.contains("ssl")
        || m.contains("tls")
        || m.contains("certificate")
        || m.contains("handshake")
    {
        return Some(ErrorKind::TlsOther);
    }
    if m.contains("connection refused") {
        return Some(ErrorKind::Refused);
    }
    if m.contains("connection reset") || m.contains("broken pipe") {
        return Some(ErrorKind::Reset);
    }
    if m.contains("network is unreachable")
        || m.contains("host is unreachable")
        || m.contains("no route to host")
    {
        return Some(ErrorKind::Unreachable);
    }
    None
}

fn is_redacted_name(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "set-cookie"
            | "x-api-key"
            | "x-auth-token"
    )
}

struct PreparedRequest<'a> {
    method: Method,
    url: Url,
    headers: &'a HeaderMap,
    secret_names: &'a [HeaderName],
}

impl fmt::Debug for PreparedRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field(
                "headers",
                &RedactedHeaders {
                    headers: self.headers,
                    secret_names: self.secret_names,
                },
            )
            .finish()
    }
}

struct RedactedHeaders<'a> {
    headers: &'a HeaderMap,
    secret_names: &'a [HeaderName],
}

impl fmt::Debug for RedactedHeaders<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut map = f.debug_map();
        for (name, value) in self.headers {
            if is_redacted_name(name) || self.secret_names.iter().any(|secret| secret == name) {
                map.entry(&name.as_str(), &"<redacted>");
            } else {
                map.entry(&name.as_str(), &value);
            }
        }
        map.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    #[test]
    fn https_to_http_is_downgrade() {
        assert_eq!(
            decide_redirect(
                &url("https://a.example/health"),
                Some("http://a.example/health"),
                0
            ),
            RedirectDecision::Downgrade
        );
    }

    #[test]
    fn http_to_https_follows_and_strips() {
        match decide_redirect(
            &url("http://a.example/health"),
            Some("https://a.example/health"),
            0,
        ) {
            RedirectDecision::Follow { strip, next } => {
                assert!(strip);
                assert_eq!(next.scheme(), "https");
            }
            other => panic!("expected follow, got {other:?}"),
        }
    }

    #[test]
    fn cross_host_strips_same_scheme() {
        match decide_redirect(
            &url("https://a.example/from"),
            Some("https://b.example/to"),
            0,
        ) {
            RedirectDecision::Follow { strip, .. } => assert!(strip),
            other => panic!("expected follow, got {other:?}"),
        }
    }

    #[test]
    fn same_host_same_scheme_keeps_headers() {
        match decide_redirect(&url("https://a.example/from"), Some("/to"), 0) {
            RedirectDecision::Follow { strip, next } => {
                assert!(!strip);
                assert_eq!(next.path(), "/to");
            }
            other => panic!("expected follow, got {other:?}"),
        }
    }

    #[test]
    fn fourth_hop_is_too_many() {
        assert_eq!(
            decide_redirect(&url("https://a.example/a"), Some("/b"), MAX_REDIRECTS),
            RedirectDecision::TooMany
        );
    }

    #[test]
    fn file_location_is_invalid() {
        assert_eq!(
            decide_redirect(&url("https://a.example/a"), Some("file:///etc/passwd"), 0),
            RedirectDecision::InvalidTarget
        );
    }

    #[test]
    fn missing_location_is_not_redirect() {
        assert_eq!(
            decide_redirect(&url("https://a.example/a"), None, 0),
            RedirectDecision::NotRedirect
        );
    }

    #[test]
    fn classify_transport_messages() {
        assert_eq!(
            classify_message("failed to lookup address information: nodename nor servname"),
            Some(ErrorKind::Dns)
        );
        assert_eq!(
            classify_message("certificate verify failed: self signed certificate"),
            Some(ErrorKind::TlsUntrusted)
        );
        assert_eq!(
            classify_message("certificate has expired"),
            Some(ErrorKind::TlsExpired)
        );
        assert_eq!(
            classify_message("TLS: hostname mismatch"),
            Some(ErrorKind::TlsHostname)
        );
        assert_eq!(
            classify_message("Connection refused (os error 61)"),
            Some(ErrorKind::Refused)
        );
        assert_eq!(
            classify_message("connection reset by peer"),
            Some(ErrorKind::Reset)
        );
        assert_eq!(
            classify_message("operation timed out"),
            Some(ErrorKind::Timeout)
        );
    }

    #[test]
    fn debug_redacts_secret_and_denylist_headers() {
        let mut headers = HeaderMap::new();
        let mut api = HeaderValue::from_static("super-secret");
        api.set_sensitive(true);
        headers.insert(HeaderName::from_static("x-api-key"), api);
        headers.insert("x-trace", HeaderValue::from_static("abc"));
        let secret = HeaderName::from_static("x-custom-token");
        headers.insert(secret.clone(), HeaderValue::from_static("s3cret-value"));
        let rendered = format!(
            "{:?}",
            PreparedRequest {
                method: Method::GET,
                url: url("https://example.com/health"),
                headers: &headers,
                secret_names: &[secret],
            }
        );
        assert!(!rendered.contains("super-secret"), "{rendered}");
        assert!(!rendered.contains("s3cret-value"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
        assert!(rendered.contains("abc"), "{rendered}");
    }

    #[test]
    fn preview_is_lossy_utf8_and_2kb() {
        let mut body = vec![0xff, 0xfe];
        body.extend(std::iter::repeat_n(b'a', 3000));
        let preview = preview_lossy(&body);
        assert!(preview.starts_with('\u{FFFD}'));
        assert!(preview.chars().count() <= BODY_PREVIEW_BYTES);
    }
}
