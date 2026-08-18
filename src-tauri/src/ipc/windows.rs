use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

const EDITOR_LABEL: &str = "editor";
const UTILITY_LABELS: &[&str] = &["editor", "detail", "settings"];

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorTarget {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// Created on open, destroyed on close — not listed in tauri.conf.json.
pub fn open_editor(app: &AppHandle, id: Option<String>) -> Result<(), String> {
    let target = EditorTarget { id: id.clone() };
    let title = if id.is_some() {
        "Edit service"
    } else {
        "Add service"
    };

    if let Some(existing) = app.get_webview_window(EDITOR_LABEL) {
        let _ = existing.set_title(title);
        let _ = existing.emit("pulse://editor-target", &target);
        let _ = existing.unminimize();
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        // Accessory apps do not raise new windows unless we go Regular first.
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    }

    let mut path = String::from("index.html?surface=editor");
    if let Some(id) = &id {
        path.push_str("&id=");
        path.push_str(id);
    }

    let window = WebviewWindowBuilder::new(app, EDITOR_LABEL, WebviewUrl::App(path.into()))
        .title(title)
        .inner_size(440.0, 640.0)
        .min_inner_size(400.0, 480.0)
        .resizable(true)
        .visible(true)
        .center()
        .build()
        .map_err(|error| error.to_string())?;

    let _ = window.emit("pulse://editor-target", &target);
    let _ = window.set_focus();
    Ok(())
}

pub fn close_editor(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(EDITOR_LABEL) {
        window.destroy().map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub fn on_window_event<R: tauri::Runtime>(window: &tauri::Window<R>, event: &tauri::WindowEvent) {
    if !UTILITY_LABELS.contains(&window.label()) {
        return;
    }
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        // Default close can keep the WebView resident; we destroy instead.
        api.prevent_close();
        let _ = window.destroy();
    }
    if matches!(event, tauri::WindowEvent::Destroyed) {
        restore_accessory_if_idle(window.app_handle());
    }
}

fn restore_accessory_if_idle<R: tauri::Runtime>(app: &AppHandle<R>) {
    let any_utility = UTILITY_LABELS
        .iter()
        .any(|label| app.get_webview_window(label).is_some());
    if any_utility {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
}

#[cfg(test)]
mod tests {
    use super::UTILITY_LABELS;

    #[test]
    fn editor_is_a_utility_window() {
        assert!(UTILITY_LABELS.contains(&"editor"));
    }
}
