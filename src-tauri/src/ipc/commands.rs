use std::sync::{Arc, Mutex};

use tauri::{AppHandle, State, WebviewWindow};

use crate::domain::{AppSettings, CheckEvidence, CheckResult, ServiceDraft, ServiceView};
use crate::ipc::draft::run_test_draft;
use chrono::{DateTime, Utc};
use serde::Serialize;
use tauri::{State, WebviewWindow};

use crate::domain::{CheckResult, CompactSample, ServiceView};
use crate::poller::scheduler::{SchedulerError, SchedulerHandle};
use crate::poller::HttpClient;
use crate::store::secrets::ensure_reveal_window;
use crate::store::{BeginRevealResponse, ConfigStore, RevealError, RevealRegistry, SecretStore};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailPayload {
    pub view: ServiceView,
    pub last: Option<CheckResult>,
    pub samples24h: Vec<CompactSample>,
}

pub struct AppState {
    pub store: Mutex<ConfigStore>,
    pub secrets: Arc<SecretStore>,
    pub reveals: Mutex<RevealRegistry>,
    pub scheduler: SchedulerHandle,
    pub http: HttpClient,
}

impl AppState {
    pub fn new(store: ConfigStore, secrets: Arc<SecretStore>, scheduler: SchedulerHandle) -> Self {
        Self {
            store: Mutex::new(store),
            secrets,
            reveals: Mutex::new(RevealRegistry::new()),
            scheduler,
            http: HttpClient::new(),
        }
    }
}

fn secret_header_exists(
    store: &ConfigStore,
    service_id: &str,
    header_key: &str,
) -> Result<(), RevealError> {
    let services = store
        .load_services()
        .map_err(|error| RevealError::Store(error.to_string()))?;
    let service = services
        .iter()
        .find(|service| service.id == service_id)
        .ok_or(RevealError::NotFound)?;
    if service
        .headers
        .iter()
        .any(|header| header.secret && header.key.eq_ignore_ascii_case(header_key))
    {
        Ok(())
    } else {
        Err(RevealError::NotFound)
    }
}

#[tauri::command(rename_all = "camelCase")]
pub fn begin_reveal(
    window: WebviewWindow,
    state: State<'_, AppState>,
    id: String,
    header_key: String,
) -> Result<BeginRevealResponse, RevealError> {
    ensure_reveal_window(window.label())?;
    let store = state.store.lock().expect("config store lock");
    secret_header_exists(&store, &id, &header_key)?;
    Ok(state
        .reveals
        .lock()
        .expect("reveal registry lock")
        .begin(&id, &header_key))
}

#[tauri::command(rename_all = "camelCase")]
pub fn reveal_secret(
    window: WebviewWindow,
    state: State<'_, AppState>,
    id: String,
    header_key: String,
    token: String,
) -> Result<String, RevealError> {
    ensure_reveal_window(window.label())?;
    state.reveals.lock().expect("reveal registry lock").reveal(
        &token,
        &id,
        &header_key,
        &state.secrets,
    )
}

#[tauri::command(rename_all = "camelCase")]
pub fn end_reveal(
    window: WebviewWindow,
    state: State<'_, AppState>,
    token: String,
) -> Result<(), RevealError> {
    ensure_reveal_window(window.label())?;
    state
        .reveals
        .lock()
        .expect("reveal registry lock")
        .end(&token);
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_services(state: State<'_, AppState>) -> Vec<ServiceView> {
    state.scheduler.views()
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_settings(state: State<'_, AppState>) -> Result<AppSettings, SchedulerError> {
    Ok(state
        .store
        .lock()
        .expect("config store lock")
        .load_settings()?)
}

#[tauri::command(rename_all = "camelCase")]
pub fn save_service(
    state: State<'_, AppState>,
    draft: ServiceDraft,
) -> Result<ServiceView, SchedulerError> {
    let service = {
        let store = state.store.lock().expect("config store lock");
        store.save_service(&state.secrets, draft)?
    };
    let id = service.id.clone();
    state.scheduler.upsert(service);
    state.scheduler.view(&id)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn test_draft(
    state: State<'_, AppState>,
    draft: ServiceDraft,
) -> Result<CheckEvidence, SchedulerError> {
    // Resolve secrets on the Rust side. The editor must not call reveal_secret.
    Ok(run_test_draft(&state.secrets, &state.http, draft).await)
}

#[tauri::command(rename_all = "camelCase")]
pub fn open_editor(app: AppHandle, id: Option<String>) -> Result<(), String> {
    crate::ipc::windows::open_editor(&app, id)
}

#[tauri::command(rename_all = "camelCase")]
pub fn close_editor(app: AppHandle) -> Result<(), String> {
    crate::ipc::windows::close_editor(&app)
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_paused(
    state: State<'_, AppState>,
    id: String,
    paused: bool,
) -> Result<ServiceView, SchedulerError> {
    state
        .store
        .lock()
        .expect("config store lock")
        .set_paused(&id, paused)?;
    state.scheduler.set_paused(&id, paused)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn check_now(
    state: State<'_, AppState>,
    id: String,
) -> Result<CheckResult, SchedulerError> {
    state.scheduler.check_now(&id).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn check_all(state: State<'_, AppState>) -> Result<(), SchedulerError> {
    state.scheduler.check_all().await;
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_detail(state: State<'_, AppState>, id: String) -> Result<DetailPayload, SchedulerError> {
    let view = state.scheduler.view(&id)?;
    let last = view.last_result.clone();
    let samples24h = state
        .scheduler
        .with_history(|history| history.samples_24h(&id, Utc::now()))?;
    Ok(DetailPayload {
        view,
        last,
        samples24h,
    })
}

#[tauri::command(rename_all = "camelCase")]
pub fn snooze(
    state: State<'_, AppState>,
    id: String,
    until: Option<String>,
) -> Result<ServiceView, SchedulerError> {
    let until = parse_snooze_until(until)?;
    state.scheduler.set_snooze(&id, until)
}

#[tauri::command(rename_all = "camelCase")]
pub fn open_action(state: State<'_, AppState>, id: String) -> Result<(), SchedulerError> {
    let view = state.scheduler.view(&id)?;
    let url = view
        .service
        .action_url
        .as_deref()
        .unwrap_or(view.service.url.as_str());
    open::that(url).map_err(|_| SchedulerError::Open)
}

#[tauri::command(rename_all = "camelCase")]
pub fn open_detail(app: tauri::AppHandle, id: String) -> Result<(), String> {
    crate::platform::detail::open_detail(&app, &id).map_err(|error| error.to_string())
}

fn parse_snooze_until(until: Option<String>) -> Result<Option<DateTime<Utc>>, SchedulerError> {
    match until {
        None => Ok(None),
        Some(value) => DateTime::parse_from_rfc3339(&value)
            .map(|parsed| Some(parsed.with_timezone(&Utc)))
            .map_err(|_| SchedulerError::InvalidSnooze),
    }
}

#[tauri::command(rename_all = "camelCase")]
pub fn delete_service(state: State<'_, AppState>, id: String) -> Result<(), SchedulerError> {
    {
        let store = state.store.lock().expect("config store lock");
        state
            .scheduler
            .with_history(|history| store.delete_service(&state.secrets, history, &id))?;
    }
    state.scheduler.remove(&id);
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub fn poller_dead(state: State<'_, AppState>) -> bool {
    state.scheduler.poller_dead()
}

#[tauri::command]
pub fn quit(app: AppHandle) {
    app.exit(0);
}

#[tauri::command(rename_all = "camelCase")]
pub fn open_action(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let view = state
        .scheduler
        .view(&id)
        .map_err(|error| error.to_string())?;
    let url = view
        .service
        .action_url
        .as_deref()
        .unwrap_or(view.service.url.as_str());
    open_http_url(url)
}

fn open_http_url(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|error| error.to_string())?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("unsupported url scheme".into());
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(parsed.as_str())
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", parsed.as_str()])
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = parsed;
        return Err("unsupported platform".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::secret_header_exists;
    use crate::domain::{HeaderSpec, HttpMethod, Service};
    use crate::store::{ConfigStore, Paths, RevealError};
    use chrono::Utc;

    fn sample(id: &str) -> Service {
        Service {
            id: id.to_string(),
            name: "Payments".into(),
            url: "https://pay.example/health".into(),
            method: HttpMethod::Get,
            headers: vec![HeaderSpec {
                key: "Authorization".into(),
                secret: true,
                value: None,
            }],
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
    fn parse_snooze_until_accepts_rfc3339_or_null() {
        assert_eq!(super::parse_snooze_until(None).unwrap(), None);
        let parsed = super::parse_snooze_until(Some("2026-08-19T08:00:00.000Z".into()))
            .unwrap()
            .unwrap();
        assert_eq!(parsed.to_rfc3339(), "2026-08-19T08:00:00+00:00");
        assert!(super::parse_snooze_until(Some("not-a-date".into())).is_err());
    }

    #[test]
    fn reveal_requires_existing_secret_header() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::open(Paths::new(dir.path())).unwrap();
        store.save_services(&[sample("svc")]).unwrap();
        assert!(secret_header_exists(&store, "svc", "Authorization").is_ok());
        assert!(matches!(
            secret_header_exists(&store, "svc", "Accept"),
            Err(RevealError::NotFound)
        ));
        assert!(matches!(
            secret_header_exists(&store, "missing", "Authorization"),
            Err(RevealError::NotFound)
        ));
    }

    #[test]
    fn open_http_url_rejects_non_http() {
        assert!(super::open_http_url("file:///etc/passwd").is_err());
        assert!(super::open_http_url("not a url").is_err());
    }
}
