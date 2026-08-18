use std::sync::Mutex;

use tauri::{State, WebviewWindow};

use crate::store::secrets::ensure_reveal_window;
use crate::store::{BeginRevealResponse, ConfigStore, RevealError, RevealRegistry, SecretStore};

pub struct AppState {
    pub store: Mutex<ConfigStore>,
    pub secrets: SecretStore,
    pub reveals: Mutex<RevealRegistry>,
}

impl AppState {
    pub fn new(store: ConfigStore) -> Self {
        Self {
            store: Mutex::new(store),
            secrets: SecretStore::new(),
            reveals: Mutex::new(RevealRegistry::new()),
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
}
