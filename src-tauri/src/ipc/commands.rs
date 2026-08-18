use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, State, WebviewWindow};

use crate::domain::{AppSettings, CheckEvidence, CheckResult, ServiceDraft, ServiceView};
use crate::ipc::draft::run_test_draft;
use chrono::{DateTime, Utc};
use serde::Serialize;
use tauri::{State, WebviewWindow};

use crate::domain::{CheckResult, CompactSample, ServiceView};

use crate::domain::{CheckResult, ServiceDraft, ServiceView};
use crate::notify::request_permission_on_notify_save;
use crate::domain::{AppSettings, CheckResult, ServiceView};
use chrono::{DateTime, Utc};
use serde::Serialize;
use tauri::{AppHandle, Manager, State, WebviewWindow};

use crate::domain::{
    AppSettings, CheckEvidence, CheckResult, CompactSample, ServiceDraft, ServiceView,
};
use crate::ipc::draft::run_test_draft;
use crate::notify::{request_permission_on_notify_save, NotifyHub};
use crate::poller::scheduler::{SchedulerError, SchedulerHandle};
use crate::poller::HttpClient;
use crate::store::secrets::ensure_reveal_window;
use crate::store::{
    confirm_message, export_filename, BeginRevealResponse, ConfigStore, ImportOutcome, RevealError,
    RevealRegistry, SecretStore,
};

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

/// Persist a draft, start polling, and prompt for notification permission
/// on the first save of a service with `notify: true` (not at launch).
#[tauri::command(rename_all = "camelCase")]
pub fn save_service(
    app: AppHandle,
    state: State<'_, AppState>,
    draft: ServiceDraft,
) -> Result<ServiceView, SchedulerError> {
    let notify = draft.notify;
    let is_create = draft.id.is_none();
    let service = state
        .store
        .lock()
        .expect("config store lock")
        .save_service(&state.secrets, draft)?;
    let id = service.id.clone();
    state.scheduler.upsert(service);
    if notify {
        request_permission_on_notify_save(&app);
    }
    if is_create {
        crate::platform::autostart::notify_service_created();
    }
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
pub fn get_settings(state: State<'_, AppState>) -> Result<AppSettings, SchedulerError> {
    Ok(state
        .store
        .lock()
        .expect("config store lock")
        .load_settings()?)
}

#[tauri::command(rename_all = "camelCase")]
pub fn update_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AppSettings,
) -> Result<AppSettings, String> {
    crate::platform::autostart::validate_hotkey(&settings)?;
    // Register before disk write so a failed bind keeps the last-good shortcut.
    crate::platform::autostart::apply_hotkey(&app, &settings)?;
    let mut settings = settings;
    if settings.launch_at_login {
        settings.asked_launch_at_login = true;
    }
    state
        .store
        .lock()
        .expect("config store lock")
        .save_settings(&settings)
        .map_err(|error| error.to_string())?;
    if let Some(hub) = app.try_state::<NotifyHub>() {
        hub.set_sound(settings.sound);
    }
    if settings.notifications {
        request_permission_on_notify_save(&app);
    }
    crate::platform::autostart::persist_side_effects(&app, &settings)?;
    Ok(settings)
}

#[tauri::command(rename_all = "camelCase")]
pub fn open_settings(app: AppHandle) {
    crate::platform::settings::open_settings(&app);
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
    open_http_url(url).map_err(|_| SchedulerError::Open)
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

/// First-save hook (editor / settings). Opens Settings if the prompt is still pending.
#[tauri::command(rename_all = "camelCase")]
pub fn maybe_ask_launch_at_login(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<AppSettings, String> {
    let settings = state
        .store
        .lock()
        .expect("config store lock")
        .load_settings()
        .map_err(|error| error.to_string())?;
    if crate::platform::autostart::maybe_ask_after_save(&app, &settings) {
        crate::platform::autostart::open_settings(&app);
    }
    state
        .store
        .lock()
        .expect("config store lock")
        .load_settings()
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn pending_launch_prompt() -> bool {
    crate::platform::autostart::take_pending_launch_prompt()
}

#[tauri::command(rename_all = "camelCase")]
pub fn answer_launch_prompt(app: AppHandle, enable: bool) -> Result<AppSettings, String> {
    crate::platform::autostart::answer_launch_prompt(&app, enable)
}

#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    #[error("canceled")]
    Canceled,
    #[error("{0}")]
    Store(#[from] crate::store::StoreError),
    #[error("{0}")]
    Dialog(String),
}

impl serde::Serialize for TransferError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

#[tauri::command(rename_all = "camelCase")]
pub async fn export_config(
    app: AppHandle,
    state: State<'_, AppState>,
    include_secrets: bool,
    include_settings: Option<bool>,
) -> Result<String, TransferError> {
    let include_settings = include_settings.unwrap_or(false);
    let path = save_json_path(&app, export_filename(include_secrets)).await?;
    {
        let store = state.store.lock().expect("config store lock");
        store.export_to_path(&state.secrets, &path, include_secrets, include_settings)?;
        let settings = store.load_settings().map_err(TransferError::from)?;
        crate::platform::autostart::persist_side_effects(&app, &settings)
            .map_err(TransferError::Dialog)?;
    }
    Ok(path.display().to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn import_config(
    app: AppHandle,
    state: State<'_, AppState>,
    include_secrets: bool,
    replace_settings: Option<bool>,
) -> Result<ImportOutcome, TransferError> {
    let replace_settings = replace_settings.unwrap_or(false);
    let path = pick_json_path(&app).await?;
    let bytes = std::fs::read(&path).map_err(|error| TransferError::Dialog(error.to_string()))?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("export.json")
        .to_string();
    let preview = ConfigStore::preview_import(&bytes, &filename, include_secrets)?;
    if !confirm_import(&app, &confirm_message(&preview, include_secrets)).await? {
        return Err(TransferError::Canceled);
    }
    let planned_settings = {
        let store = state.store.lock().expect("config store lock");
        store.planned_import_settings(&bytes, replace_settings)?
    };
    if let Some(settings) = &planned_settings {
        crate::platform::autostart::validate_hotkey(settings).map_err(TransferError::Dialog)?;
        crate::platform::autostart::apply_hotkey(&app, settings).map_err(TransferError::Dialog)?;
    }
    let was_empty = state.scheduler.views().is_empty();
    let (outcome, services, settings) = {
        let store = state.store.lock().expect("config store lock");
        store.import_from_bytes(&state.secrets, &bytes, include_secrets, replace_settings)?
    };
    for service in services {
        state.scheduler.upsert(service);
    }
    if was_empty && outcome.added > 0 {
        crate::platform::autostart::notify_service_created();
    }
    if let Some(settings) = settings {
        crate::platform::autostart::persist_side_effects(&app, &settings)
            .map_err(TransferError::Dialog)?;
    }
    Ok(outcome)
}

#[tauri::command(rename_all = "camelCase")]
pub fn reset_all(app: AppHandle, state: State<'_, AppState>) -> Result<(), TransferError> {
    {
        let store = state.store.lock().expect("config store lock");
        state
            .scheduler
            .with_history(|history| store.reset_all(&state.secrets, history))?;
    }
    state.scheduler.clear_services();
    let settings = AppSettings::default();
    let hotkey = crate::platform::autostart::apply_hotkey(&app, &settings);
    crate::platform::autostart::persist_side_effects(&app, &settings)
        .map_err(TransferError::Dialog)?;
    hotkey.map_err(TransferError::Dialog)?;
    Ok(())
}

async fn pick_json_path(app: &AppHandle) -> Result<PathBuf, TransferError> {
    dialog_path(app, DialogKind::Open, None).await
}

async fn save_json_path(app: &AppHandle, filename: &str) -> Result<PathBuf, TransferError> {
    dialog_path(app, DialogKind::Save, Some(filename)).await
}

enum DialogKind {
    Open,
    Save,
}

async fn dialog_path(
    app: &AppHandle,
    kind: DialogKind,
    filename: Option<&str>,
) -> Result<PathBuf, TransferError> {
    #[cfg(desktop)]
    {
        use tauri_plugin_dialog::DialogExt;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut builder = app.dialog().file().add_filter("JSON", &["json"]);
        if let Some(filename) = filename {
            builder = builder.set_file_name(filename);
        }
        match kind {
            DialogKind::Open => builder.pick_file(move |path| {
                let _ = tx.send(path);
            }),
            DialogKind::Save => builder.save_file(move |path| {
                let _ = tx.send(path);
            }),
        }
        match rx.await {
            Ok(Some(file)) => file
                .into_path()
                .map_err(|error| TransferError::Dialog(error.to_string())),
            Ok(None) | Err(_) => Err(TransferError::Canceled),
        }
    }
    #[cfg(not(desktop))]
    {
        let _ = (app, kind, filename);
        Err(TransferError::Dialog("dialogs are desktop-only".into()))
    }
}

async fn confirm_import(app: &AppHandle, message: &str) -> Result<bool, TransferError> {
    #[cfg(desktop)]
    {
        use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
        let (tx, rx) = tokio::sync::oneshot::channel();
        app.dialog()
            .message(message)
            .title("Import")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancel)
            .show(move |ok| {
                let _ = tx.send(ok);
            });
        rx.await.map_err(|_| TransferError::Canceled)
    }
    #[cfg(not(desktop))]
    {
        let _ = (app, message);
        Err(TransferError::Dialog("dialogs are desktop-only".into()))
    }
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
    use crate::domain::{AppSettings, HeaderSpec, HttpMethod, QuietHours, Service};
    use crate::notify::in_quiet_window;
    use crate::store::{ConfigStore, Paths, RevealError};
    use chrono::{NaiveDate, NaiveDateTime, Utc};

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

    fn ndt(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_opt(hour, minute, 0)
            .unwrap()
    }

    #[test]
    fn overnight_friday_saturday_via_settings_persist() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::open(Paths::new(dir.path())).unwrap();
        let settings = AppSettings {
            quiet_hours: Some(QuietHours {
                start: "22:00".into(),
                end: "08:00".into(),
                days: vec![1, 2, 3, 4, 5],
            }),
            ..AppSettings::default()
        };
        store.save_settings(&settings).unwrap();
        let hours = store.load_settings().unwrap().quiet_hours.unwrap();
        // 2026-08-21 Friday; 22nd Saturday is unchecked.
        assert!(!in_quiet_window(&hours, ndt(2026, 8, 21, 21, 59)));
        assert!(in_quiet_window(&hours, ndt(2026, 8, 21, 22, 0)));
        assert!(in_quiet_window(&hours, ndt(2026, 8, 22, 0, 0)));
        assert!(in_quiet_window(&hours, ndt(2026, 8, 22, 7, 59)));
        assert!(!in_quiet_window(&hours, ndt(2026, 8, 22, 8, 0)));
        assert!(!in_quiet_window(&hours, ndt(2026, 8, 22, 22, 0)));
        assert!(!in_quiet_window(&hours, ndt(2026, 8, 23, 7, 59)));
    }

    #[test]
    fn invalid_quiet_hours_rejected_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::open(Paths::new(dir.path())).unwrap();
        let settings = AppSettings {
            quiet_hours: Some(QuietHours {
                start: "25:00".into(),
                end: "08:00".into(),
                days: vec![1, 2, 3, 4, 5],
            }),
            ..AppSettings::default()
        };
        assert!(store.save_settings(&settings).is_err());
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
