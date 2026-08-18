//! Detail is a utility window: create on demand, destroy on close, hide the popover.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Listener, Manager, WebviewUrl, WebviewWindowBuilder};

pub const DETAIL_LABEL: &str = "detail";
pub const DETAIL_WIDTH: f64 = 420.0;
pub const DETAIL_HEIGHT: f64 = 560.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailId {
    pub id: String,
}

pub fn parse_detail_id(payload: &str) -> Option<String> {
    serde_json::from_str::<DetailId>(payload)
        .ok()
        .map(|payload| payload.id)
        .filter(|id| !id.is_empty())
}

pub fn open_detail<R: tauri::Runtime>(app: &AppHandle<R>, id: &str) -> Result<(), tauri::Error> {
    if let Some(popover) = app.get_webview_window("popover") {
        let _ = popover.hide();
    }

    if let Some(existing) = app.get_webview_window(DETAIL_LABEL) {
        let _ = existing.emit("pulse://detail-service", DetailId { id: id.to_string() });
        let _ = existing.unminimize();
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }

    let init = format!(
        "window.__PULSE_DETAIL_ID__={};",
        serde_json::to_string(id).unwrap_or_else(|_| "\"\"".into())
    );
    WebviewWindowBuilder::new(
        app,
        DETAIL_LABEL,
        WebviewUrl::App(format!("index.html?id={id}").into()),
    )
    .title("Pulse")
    .inner_size(DETAIL_WIDTH, DETAIL_HEIGHT)
    .min_inner_size(360.0, 420.0)
    .resizable(true)
    .decorations(true)
    .skip_taskbar(false)
    .visible(true)
    .initialization_script(&init)
    .build()?;
    Ok(())
}

pub fn install<R: tauri::Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    let _id = app.listen("pulse://open-detail", move |event| {
        if let Some(id) = parse_detail_id(event.payload()) {
            let _ = open_detail(&handle, &id);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::parse_detail_id;

    #[test]
    fn parse_detail_id_requires_non_empty() {
        assert_eq!(
            parse_detail_id(r#"{"id":"01JABCDEF0000000000000API"}"#).as_deref(),
            Some("01JABCDEF0000000000000API")
        );
        assert!(parse_detail_id(r#"{"id":""}"#).is_none());
        assert!(parse_detail_id("not-json").is_none());
    }
}
