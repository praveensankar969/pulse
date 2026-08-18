//! Launch-at-login, first-save prompt, global hotkey, on-demand settings window.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Listener, Manager};

use crate::domain::{
    apply_launch_prompt, launch_prompt_action, resolved_hotkey, AppSettings, LaunchPromptAction,
};

pub const SETTINGS_WIDTH: f64 = crate::platform::settings::SETTINGS_WIDTH;
pub const SETTINGS_HEIGHT: f64 = crate::platform::settings::SETTINGS_HEIGHT;

static PENDING_LAUNCH_PROMPT: AtomicBool = AtomicBool::new(false);
static LAST_HOTKEY: Mutex<Option<String>> = Mutex::new(None);
static ON_SERVICE_CREATED: Mutex<Option<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(None);

pub fn take_pending_launch_prompt() -> bool {
    PENDING_LAUNCH_PROMPT.swap(false, Ordering::SeqCst)
}

pub fn arm_launch_prompt() {
    PENDING_LAUNCH_PROMPT.store(true, Ordering::SeqCst);
}

#[cfg(desktop)]
pub fn apply_launch_at_login<R: tauri::Runtime>(app: &AppHandle<R>, enabled: bool) {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    if let Err(error) = result {
        tracing::warn!(enabled, %error, "launch at login apply failed");
    }
}

#[cfg(not(desktop))]
pub fn apply_launch_at_login<R: tauri::Runtime>(_app: &AppHandle<R>, _enabled: bool) {}

/// If bind fails, keep `previous` registered instead of dropping the shortcut.
pub fn commit_hotkey(
    previous: Option<String>,
    candidate: String,
    registered: bool,
) -> Option<String> {
    if registered {
        Some(candidate)
    } else {
        previous
    }
}

#[cfg(desktop)]
pub fn apply_hotkey<R: tauri::Runtime>(
    app: &AppHandle<R>,
    settings: &AppSettings,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

    let hotkey = resolved_hotkey(settings);
    let parsed: Shortcut = hotkey
        .parse()
        .map_err(|error| format!("invalid hotkey `{hotkey}`: {error}"))?;

    let shortcuts = app.global_shortcut();
    let previous = LAST_HOTKEY.lock().expect("hotkey lock").clone();
    if let Err(error) = shortcuts.unregister_all() {
        tracing::debug!(%error, "hotkey unregister_all");
    }

    let register = |binding: Shortcut| {
        shortcuts.on_shortcut(binding, |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                crate::platform::tray::toggle_popover(app);
            }
        })
    };

    match register(parsed) {
        Ok(()) => {
            *LAST_HOTKEY.lock().expect("hotkey lock") = commit_hotkey(previous, hotkey, true);
            Ok(())
        }
        Err(error) => {
            let restored = previous.clone().and_then(|prev| {
                prev.parse::<Shortcut>()
                    .ok()
                    .and_then(|binding| register(binding).ok().map(|_| prev))
            });
            *LAST_HOTKEY.lock().expect("hotkey lock") = commit_hotkey(restored, hotkey, false);
            Err(error.to_string())
        }
    }
}

#[cfg(not(desktop))]
pub fn apply_hotkey<R: tauri::Runtime>(
    _app: &AppHandle<R>,
    _settings: &AppSettings,
) -> Result<(), String> {
    Ok(())
}

pub fn validate_hotkey(settings: &AppSettings) -> Result<(), String> {
    #[cfg(desktop)]
    {
        use tauri_plugin_global_shortcut::Shortcut;
        let hotkey = resolved_hotkey(settings);
        let _: Shortcut = hotkey
            .parse()
            .map_err(|error| format!("invalid hotkey `{hotkey}`: {error}"))?;
    }
    let _ = settings;
    Ok(())
}

pub fn persist_side_effects<R: tauri::Runtime>(
    app: &AppHandle<R>,
    settings: &AppSettings,
) -> Result<(), String> {
    apply_launch_at_login(app, settings.launch_at_login);
    if let Some(state) = app.try_state::<crate::ipc::AppState>() {
        state.scheduler.update_settings(settings.clone());
    }
    let _ = app.emit("pulse://settings", settings);
    Ok(())
}

/// Rust-side hook from `ConfigStore::save_service` on first create.
/// Spawned so we never re-enter `AppState.store` under the caller's lock.
pub fn notify_service_created() {
    let callback = ON_SERVICE_CREATED.lock().expect("create hook lock").clone();
    let Some(callback) = callback else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        callback();
    });
}

pub fn maybe_ask_after_save<R: tauri::Runtime>(app: &AppHandle<R>, settings: &AppSettings) -> bool {
    match launch_prompt_action(settings) {
        LaunchPromptAction::Skip => false,
        LaunchPromptAction::MarkAsked => {
            let mut next = settings.clone();
            next.asked_launch_at_login = true;
            if let Some(state) = app.try_state::<crate::ipc::AppState>() {
                if state
                    .store
                    .lock()
                    .expect("config store lock")
                    .save_settings(&next)
                    .is_ok()
                {
                    let _ = persist_side_effects(app, &next);
                }
            }
            false
        }
        LaunchPromptAction::Ask => {
            arm_launch_prompt();
            let _ = app.emit("pulse://ask-launch-at-login", ());
            true
        }
    }
}

pub fn answer_launch_prompt<R: tauri::Runtime>(
    app: &AppHandle<R>,
    enable: bool,
) -> Result<AppSettings, String> {
    let state = app
        .try_state::<crate::ipc::AppState>()
        .ok_or_else(|| "app state missing".to_string())?;
    let mut settings = state
        .store
        .lock()
        .expect("config store lock")
        .load_settings()
        .map_err(|error| error.to_string())?;
    apply_launch_prompt(&mut settings, enable);
    state
        .store
        .lock()
        .expect("config store lock")
        .save_settings(&settings)
        .map_err(|error| error.to_string())?;
    persist_side_effects(app, &settings)?;
    PENDING_LAUNCH_PROMPT.store(false, Ordering::SeqCst);
    Ok(settings)
}

pub fn open_settings<R: tauri::Runtime>(app: &AppHandle<R>) {
    crate::platform::settings::open_settings(app);
}

pub fn install<R: tauri::Runtime>(app: &AppHandle<R>, settings: &AppSettings) {
    apply_launch_at_login(app, settings.launch_at_login);
    if let Err(error) = apply_hotkey(app, settings) {
        tracing::warn!(%error, "hotkey register failed");
    }

    let handle = app.clone();
    let _ = app.listen("pulse://open-settings", move |_| {
        open_settings(&handle);
    });

    let created = app.clone();
    *ON_SERVICE_CREATED.lock().expect("create hook lock") = Some(Arc::new(move || {
        let Some(state) = created.try_state::<crate::ipc::AppState>() else {
            return;
        };
        let Ok(settings) = state
            .store
            .lock()
            .expect("config store lock")
            .load_settings()
        else {
            return;
        };
        if maybe_ask_after_save(&created, &settings) {
            open_settings(&created);
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::DEFAULT_HOTKEY;

    #[test]
    fn settings_window_size_matches_design() {
        assert_eq!(SETTINGS_WIDTH, 440.0);
        assert_eq!(SETTINGS_HEIGHT, 560.0);
    }

    #[test]
    fn pending_launch_prompt_is_one_shot() {
        PENDING_LAUNCH_PROMPT.store(false, Ordering::SeqCst);
        assert!(!take_pending_launch_prompt());
        arm_launch_prompt();
        assert!(take_pending_launch_prompt());
        assert!(!take_pending_launch_prompt());
    }

    #[test]
    fn default_hotkey_parses() {
        validate_hotkey(&AppSettings {
            hotkey: Some(DEFAULT_HOTKEY.into()),
            ..AppSettings::default()
        })
        .unwrap();
    }

    #[test]
    fn failed_register_keeps_last_good_hotkey() {
        assert_eq!(
            commit_hotkey(Some("CommandOrControl+Shift+U".into()), "bad".into(), false),
            Some("CommandOrControl+Shift+U".into())
        );
        assert_eq!(
            commit_hotkey(Some("old".into()), "CommandOrControl+Shift+P".into(), true),
            Some("CommandOrControl+Shift+P".into())
        );
    }

    #[test]
    fn notify_service_created_is_noop_without_install() {
        notify_service_created();
    }
}
