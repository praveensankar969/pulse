use std::collections::HashMap;
use std::net::TcpListener;
use std::time::Duration;

use chrono::{DateTime, Utc};
use pulse_lib::domain::{ErrorKind, ExpectedStatus, HeaderSpec, HttpMethod, Service};
use pulse_lib::poller::{HttpClient, BODY_PREVIEW_BYTES, MAX_BODY_BYTES, USER_AGENT};
use wiremock::matchers::{body_string, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixed_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-08-18T14:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn service(url: String) -> Service {
    Service {
        id: "01TEST00000000000000000000".into(),
        name: "t".into(),
        url,
        method: HttpMethod::Get,
        headers: vec![],
        body: None,
        interval_sec: 60,
        timeout_ms: 5_000,
        expected_status: ExpectedStatus::TwoXx,
        assertions: vec![],
        max_latency_ms: None,
        action_url: None,
        notify: true,
        always_alert: false,
        paused: false,
        follow_redirects: true,
        fail_threshold: None,
        group: None,
        created_at: fixed_time(),
        updated_at: fixed_time(),
    }
}

/// Second host for cross-host redirect fixtures. 127.0.0.2 is not always
/// assigned; IPv6 loopback (`::1`) is a different host from `127.0.0.1`.
async fn start_other_host() -> MockServer {
    let listener = TcpListener::bind("[::1]:0").expect("bind ipv6 loopback");
    listener.set_nonblocking(true).expect("nonblocking");
    MockServer::builder().listener(listener).start().await
}

#[tokio::test]
async fn ok_returns_status_body_and_preview() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"status":"ok"}"#))
        .mount(&server)
        .await;

    let raw = HttpClient::new()
        .check(
            &service(format!("{}/health", server.uri())),
            &HashMap::new(),
        )
        .await
        .unwrap();
    assert_eq!(raw.status, 200);
    assert_eq!(raw.redirects, 0);
    assert!(!raw.headers_stripped_on_redirect);
    assert_eq!(raw.body, br#"{"status":"ok"}"#);
    assert_eq!(raw.body_preview, r#"{"status":"ok"}"#);
}

#[tokio::test]
async fn sends_pulse_user_agent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ua"))
        .and(header("user-agent", USER_AGENT))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let raw = HttpClient::new()
        .check(&service(format!("{}/ua", server.uri())), &HashMap::new())
        .await
        .unwrap();
    assert_eq!(raw.status, 200);
}

#[tokio::test]
async fn follow_redirects_false_evaluates_first_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/moved"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/final"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/final"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let mut svc = service(format!("{}/moved", server.uri()));
    svc.follow_redirects = false;
    let raw = HttpClient::new()
        .check(&svc, &HashMap::new())
        .await
        .unwrap();
    assert_eq!(raw.status, 302);
    assert_eq!(raw.redirects, 0);
}

#[tokio::test]
async fn same_host_redirect_keeps_x_api_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/from"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", format!("{}/to", server.uri())),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/to"))
        .and(header("x-api-key", "keep-me"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .expect(1)
        .mount(&server)
        .await;

    let mut svc = service(format!("{}/from", server.uri()));
    svc.headers = vec![HeaderSpec {
        key: "X-Api-Key".into(),
        secret: true,
        value: None,
    }];
    let mut secrets = HashMap::new();
    secrets.insert("X-Api-Key".into(), "keep-me".into());

    let raw = HttpClient::new().check(&svc, &secrets).await.unwrap();
    assert_eq!(raw.status, 200);
    assert_eq!(raw.redirects, 1);
    assert!(!raw.headers_stripped_on_redirect);
}

#[tokio::test]
async fn cross_host_302_does_not_forward_x_api_key() {
    let origin = MockServer::start().await;
    let target = start_other_host().await;

    Mock::given(method("GET"))
        .and(path("/from"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", format!("{}/to", target.uri())),
        )
        .mount(&origin)
        .await;
    Mock::given(method("GET"))
        .and(path("/to"))
        .respond_with(ResponseTemplate::new(200).set_body_string("landed"))
        .expect(1)
        .mount(&target)
        .await;

    let mut svc = service(format!("{}/from", origin.uri()));
    svc.headers = vec![
        HeaderSpec {
            key: "X-Api-Key".into(),
            secret: true,
            value: None,
        },
        HeaderSpec {
            key: "Authorization".into(),
            secret: true,
            value: None,
        },
        HeaderSpec {
            key: "X-Trace".into(),
            secret: false,
            value: Some("keep".into()),
        },
    ];
    let mut secrets = HashMap::new();
    secrets.insert("X-Api-Key".into(), "must-not-leak".into());
    secrets.insert("Authorization".into(), "Bearer secret".into());

    let raw = HttpClient::new().check(&svc, &secrets).await.unwrap();
    assert_eq!(raw.status, 200);
    assert_eq!(raw.redirects, 1);
    assert!(raw.headers_stripped_on_redirect);

    let received = target.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    assert!(
        received[0].headers.get("x-api-key").is_none(),
        "cross-host 302 must not forward X-Api-Key: {:?}",
        received[0].headers
    );
    assert!(received[0].headers.get("authorization").is_none());
    assert_eq!(
        received[0]
            .headers
            .get("x-trace")
            .map(|value| value.to_str().unwrap()),
        Some("keep")
    );
}

#[tokio::test]
async fn denylist_stripped_on_host_change() {
    let origin = MockServer::start().await;
    let target = start_other_host().await;
    Mock::given(method("GET"))
        .and(path("/from"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", format!("{}/to", target.uri())),
        )
        .mount(&origin)
        .await;
    Mock::given(method("GET"))
        .and(path("/to"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&target)
        .await;

    let mut svc = service(format!("{}/from", origin.uri()));
    svc.headers = vec![
        HeaderSpec {
            key: "Cookie".into(),
            secret: false,
            value: Some("sid=1".into()),
        },
        HeaderSpec {
            key: "X-Auth-Token".into(),
            secret: false,
            value: Some("tok".into()),
        },
        HeaderSpec {
            key: "Proxy-Authorization".into(),
            secret: false,
            value: Some("Basic abc".into()),
        },
    ];

    let raw = HttpClient::new()
        .check(&svc, &HashMap::new())
        .await
        .unwrap();
    assert_eq!(raw.status, 401);
    assert!(raw.headers_stripped_on_redirect);

    let hop = &target.received_requests().await.unwrap()[0];
    assert!(hop.headers.get("cookie").is_none());
    assert!(hop.headers.get("x-auth-token").is_none());
    assert!(hop.headers.get("proxy-authorization").is_none());
}

#[tokio::test]
async fn follows_three_redirects() {
    let server = MockServer::start().await;
    for (from, to) in [("/a", "/b"), ("/b", "/c"), ("/c", "/d")] {
        Mock::given(method("GET"))
            .and(path(from))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", to))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/d"))
        .respond_with(ResponseTemplate::new(200).set_body_string("done"))
        .mount(&server)
        .await;

    let raw = HttpClient::new()
        .check(&service(format!("{}/a", server.uri())), &HashMap::new())
        .await
        .unwrap();
    assert_eq!(raw.status, 200);
    assert_eq!(raw.redirects, 3);
}

#[tokio::test]
async fn fourth_redirect_is_too_many() {
    let server = MockServer::start().await;
    for (from, to) in [("/a", "/b"), ("/b", "/c"), ("/c", "/d"), ("/d", "/e")] {
        Mock::given(method("GET"))
            .and(path(from))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", to))
            .mount(&server)
            .await;
    }

    let err = HttpClient::new()
        .check(&service(format!("{}/a", server.uri())), &HashMap::new())
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::TooManyRedirects);
}

#[tokio::test]
async fn missing_secret_does_not_send() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let mut svc = service(format!("{}/health", server.uri()));
    svc.headers = vec![HeaderSpec {
        key: "X-Api-Key".into(),
        secret: true,
        value: None,
    }];
    let err = HttpClient::new()
        .check(&svc, &HashMap::new())
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::MissingSecret);
    assert_eq!(err.detail.as_deref(), Some("X-Api-Key"));
    assert!(err.latency_ms.is_none());
}

#[tokio::test]
async fn injects_secret_from_provided_map() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/s"))
        .and(header("x-api-key", "from-keychain"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let mut svc = service(format!("{}/s", server.uri()));
    svc.headers = vec![HeaderSpec {
        key: "X-Api-Key".into(),
        secret: true,
        value: None,
    }];
    let mut secrets = HashMap::new();
    secrets.insert("X-Api-Key".into(), "from-keychain".into());
    let raw = HttpClient::new().check(&svc, &secrets).await.unwrap();
    assert_eq!(raw.status, 200);
}

#[tokio::test]
async fn rejects_non_http_schemes() {
    for url in [
        "file:///tmp/x",
        "ftp://example.com/",
        "",
        "unix:///tmp.sock",
    ] {
        let err = HttpClient::new()
            .check(&service(url.into()), &HashMap::new())
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidUrl, "{url}");
    }
}

#[tokio::test]
async fn caps_body_at_64kb_and_preview_at_2kb() {
    let server = MockServer::start().await;
    let payload = vec![b'x'; 70_000];
    Mock::given(method("GET"))
        .and(path("/big"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(payload))
        .mount(&server)
        .await;

    let raw = HttpClient::new()
        .check(&service(format!("{}/big", server.uri())), &HashMap::new())
        .await
        .unwrap();
    assert_eq!(raw.body.len(), MAX_BODY_BYTES);
    assert_eq!(raw.body_preview.len(), BODY_PREVIEW_BYTES);
    assert!(raw.body.iter().all(|b| *b == b'x'));
}

#[tokio::test]
async fn timeout_maps_to_error_kind() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
        .mount(&server)
        .await;

    let mut svc = service(format!("{}/slow", server.uri()));
    svc.timeout_ms = 500;
    let err = HttpClient::new()
        .check(&svc, &HashMap::new())
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Timeout);
    assert!(err.latency_ms.is_some());
}

#[tokio::test]
async fn cookies_are_not_stored_across_redirects() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", "/next")
                .insert_header("Set-Cookie", "sid=abc"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/next"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    HttpClient::new()
        .check(&service(format!("{}/start", server.uri())), &HashMap::new())
        .await
        .unwrap();

    let hop = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|req| req.url.path() == "/next")
        .unwrap();
    assert!(
        hop.headers.get("cookie").is_none(),
        "cookie jar must stay disabled: {:?}",
        hop.headers
    );
}

#[tokio::test]
async fn post_sends_body_without_inferred_content_type() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/ingest"))
        .and(body_string("raw-bytes"))
        .respond_with(ResponseTemplate::new(201))
        .expect(1)
        .mount(&server)
        .await;

    let mut svc = service(format!("{}/ingest", server.uri()));
    svc.method = HttpMethod::Post;
    svc.body = Some("raw-bytes".into());
    let raw = HttpClient::new()
        .check(&svc, &HashMap::new())
        .await
        .unwrap();
    assert_eq!(raw.status, 201);
}

#[tokio::test]
async fn refused_maps_to_error_kind() {
    let err = HttpClient::new()
        .check(&service("http://127.0.0.1:1/".into()), &HashMap::new())
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
}

#[tokio::test]
async fn dns_failure_maps_to_error_kind() {
    let err = HttpClient::new()
        .check(
            &service("http://no-such-host.invalid/".into()),
            &HashMap::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Dns);
}
