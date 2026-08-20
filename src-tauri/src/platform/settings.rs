//! On-demand settings window. Destroyed on close; quiet-hours form lives here.

use tauri::{AppHandle, Listener, Manager, WebviewUrl, WebviewWindowBuilder};

pub const SETTINGS_LABEL: &str = "settings";
pub const SETTINGS_WIDTH: f64 = 440.0;
pub const SETTINGS_HEIGHT: f64 = 560.0;

pub fn open_settings<R: tauri::Runtime>(app: &AppHandle<R>) {
    crate::ipc::windows::become_regular(app);
    if let Some(window) = app.get_webview_window(SETTINGS_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }

    if let Err(error) = WebviewWindowBuilder::new(
        app,
        SETTINGS_LABEL,
        WebviewUrl::App("index.html?window=settings".into()),
    )
    .title("Settings")
    .inner_size(SETTINGS_WIDTH, SETTINGS_HEIGHT)
    .min_inner_size(400.0, 480.0)
    .resizable(true)
    .maximizable(false)
    .skip_taskbar(false)
    .visible(true)
    .focused(true)
    .build()
    {
        tracing::warn!(%error, "settings window create failed");
    }
}

pub fn install<R: tauri::Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    let _ = app.listen("pulse://open-settings", move |_| {
        open_settings(&handle);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_window_size() {
        assert_eq!(SETTINGS_WIDTH, 440.0);
        assert_eq!(SETTINGS_HEIGHT, 560.0);
    }
}
