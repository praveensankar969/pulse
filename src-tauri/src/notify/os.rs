//! OS toasts via `tauri-plugin-notification` / notify-rust.
//!
//! Plugin `NotificationBuilder::show` is fire-and-forget (no desktop click
//! payload; "actions" are mobile-only). We send through notify-rust — the same
//! backend the plugin uses — and `wait_for_response` shows the popover.
//! Windows body-click is `NotificationResponse::Default` (not `"__closed"`).
//! `RunEvent::Reopen` is only a Dock-relaunch fallback; this app is an
//! accessory / LSUIElement and has no Dock icon, so banner click does not go
//! through Reopen. No quiet-hours flush.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_notification::NotificationExt;

use crate::notify::{Notification, Notifier};
use crate::platform::tray;

/// Default banner / toast sound. OS may ignore this (Focus Assist, etc.).
const DEFAULT_SOUND: &str = "default";

/// Launch arg honored on an installed Windows build (`pulse:focus?id=`).
pub const FOCUS_LAUNCH: &str = "pulse:focus";

#[derive(Debug, Clone, Serialize)]
pub struct FocusServicePayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// Shared click / permission / sound state. Process-local; the OS remembers the prompt.
#[derive(Clone)]
pub struct NotifyHub {
    last_id: Arc<Mutex<Option<String>>>,
    asked_permission: Arc<AtomicBool>,
    sound: Arc<AtomicBool>,
}

impl Default for NotifyHub {
    fn default() -> Self {
        Self::new(true)
    }
}

impl NotifyHub {
    pub fn new(sound: bool) -> Self {
        Self {
            last_id: Arc::new(Mutex::new(None)),
            asked_permission: Arc::new(AtomicBool::new(false)),
            sound: Arc::new(AtomicBool::new(sound)),
        }
    }

    pub fn set_sound(&self, sound: bool) {
        self.sound.store(sound, Ordering::SeqCst);
    }

    pub fn sound(&self) -> bool {
        self.sound.load(Ordering::SeqCst)
    }

    pub fn last_id(&self) -> Option<String> {
        self.last_id.lock().expect("last notify id").clone()
    }

    pub fn remember(&self, notification: &Notification) {
        *self.last_id.lock().expect("last notify id") = last_notified_service_id(notification);
    }

    /// First notify-enabled save in this process, and not yet prompted.
    pub fn should_request_permission(&self, notify_enabled: bool) -> bool {
        should_request_permission(notify_enabled, &self.asked_permission)
    }
}

/// Single-service toast stashes the id. Digest has no per-service id.
pub fn last_notified_service_id(notification: &Notification) -> Option<String> {
    notification.service_id().map(str::to_string)
}

/// `settings.notifications && service.notify` is decided by the state machine.
/// This is only "should we ask the OS", and only once per process.
pub fn should_request_permission(notify_enabled: bool, asked: &AtomicBool) -> bool {
    if !notify_enabled {
        return false;
    }
    !asked.swap(true, Ordering::SeqCst)
}

/// `pulse:focus` / `pulse:focus?id=` / `pulse://focus?id=`.
/// `Some(None)` = show popover, no id. `None` = not a focus launch.
pub fn parse_focus_args<S: AsRef<str>>(args: &[S]) -> Option<Option<String>> {
    args.iter().find_map(|arg| parse_focus_arg(arg.as_ref()))
}

pub fn parse_focus_arg(arg: &str) -> Option<Option<String>> {
    let rest = arg
        .strip_prefix(FOCUS_LAUNCH)
        .or_else(|| arg.strip_prefix("pulse://focus"))?;
    if rest.is_empty() {
        return Some(None);
    }
    let rest = rest.strip_prefix('?')?;
    for pair in rest.split('&') {
        if let Some(id) = pair.strip_prefix("id=") {
            if id.is_empty() {
                return Some(None);
            }
            return Some(Some(id.to_string()));
        }
    }
    Some(None)
}

/// Banner / toast click vs dismiss. notify-rust has no service-id payload.
///
/// Windows body-click is [`notify_rust::NotificationResponse::Default`]
/// (`on_activated(None)`). `wait_for_action` maps that to `"__closed"`;
/// use [`wait_for_response`](notify_rust::NotificationHandle::wait_for_response).
pub fn is_click_response(response: &notify_rust::NotificationResponse) -> bool {
    matches!(
        response,
        notify_rust::NotificationResponse::Default
            | notify_rust::NotificationResponse::Action(_)
            | notify_rust::NotificationResponse::Reply(_)
    )
}

/// Best-effort: show popover, emit `pulse://focus-service`. Do not open detail.
pub fn handle_toast_click<R: Runtime>(app: &AppHandle<R>, id: Option<String>) {
    #[cfg(target_os = "macos")]
    {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
    tray::show_popover_if_hidden(app);
    let _ = app.emit("pulse://focus-service", FocusServicePayload { id });
}

/// Startup / single-instance: honor `pulse:focus?id=` if present.
///
/// A second-instance launch with no focus arg still shows the popover (Windows
/// toast click on an installed build often just relaunches the AUMID).
pub fn handle_activation<R: Runtime>(app: &AppHandle<R>, args: &[String]) {
    let id = match parse_focus_args(args) {
        Some(id) => id,
        None => app.try_state::<NotifyHub>().and_then(|hub| hub.last_id()),
    };
    handle_toast_click(app, id);
}

/// Plugin `request_permission` (Granted no-op on desktop) plus a real macOS prompt.
pub fn request_permission_on_notify_save<R: Runtime>(app: &AppHandle<R>) {
    let Some(hub) = app.try_state::<NotifyHub>() else {
        return;
    };
    if !hub.should_request_permission(true) {
        return;
    }
    let _ = app.notification().request_permission();
    #[cfg(target_os = "macos")]
    {
        let app = app.clone();
        let _ = app.run_on_main_thread(request_macos_authorization);
    }
}

#[cfg(target_os = "macos")]
fn request_macos_authorization() {
    use block2::RcBlock;
    use objc2_user_notifications::{UNAuthorizationOptions, UNUserNotificationCenter};

    let center = UNUserNotificationCenter::currentNotificationCenter();
    let block = RcBlock::new(|_granted, _error| {});
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &block,
    );
}

pub struct OsNotifier<R: Runtime> {
    app: AppHandle<R>,
    hub: NotifyHub,
}

impl<R: Runtime> OsNotifier<R> {
    pub fn new(app: AppHandle<R>, hub: NotifyHub) -> Self {
        Self { app, hub }
    }

    fn sound_enabled(&self) -> bool {
        self.hub.sound()
    }
}

impl<R: Runtime> Notifier for OsNotifier<R> {
    fn notify(&mut self, notification: Notification) {
        self.hub.remember(&notification);
        let (kind, title, body) = match &notification {
            Notification::Down { title, body, .. } => ("down", title.clone(), body.clone()),
            Notification::Recovered { title, body, .. } => {
                ("recovered", title.clone(), body.clone())
            }
            Notification::Digest { title, body, .. } => ("digest", title.clone(), body.clone()),
        };
        tracing::info!(event = "notify", kind, "os toast");

        let identifier = self.app.config().identifier.clone();
        let sound = self.sound_enabled();
        let app = self.app.clone();
        let hub = self.hub.clone();
        // Plugin show() drops the handle. One notify-rust toast + wait_for_response.
        tauri::async_runtime::spawn_blocking(move || {
            deliver_toast(&identifier, &title, &body, sound, &app, &hub);
        });
    }
}

/// Same notify-rust path as the plugin's desktop `show`, plus a click wait.
fn deliver_toast<R: Runtime>(
    identifier: &str,
    title: &str,
    body: &str,
    sound: bool,
    app: &AppHandle<R>,
    hub: &NotifyHub,
) {
    let notification = build_native(identifier, title, body, sound);
    match notification.show() {
        Ok(handle) => {
            let _ = handle.wait_for_response(ToastClick {
                app: app.clone(),
                hub: hub.clone(),
            });
        }
        Err(error) => tracing::warn!(error = %error, "os toast failed"),
    }
}

/// Owned handler — closures fail the HRTB on `FnOnce(&NotificationResponse)`.
struct ToastClick<R: Runtime> {
    app: AppHandle<R>,
    hub: NotifyHub,
}

impl<R: Runtime> notify_rust::ResponseHandler for ToastClick<R> {
    fn call(self, response: &notify_rust::NotificationResponse) {
        if is_click_response(response) {
            handle_toast_click(&self.app, self.hub.last_id());
        }
    }
}

fn build_native(
    identifier: &str,
    title: &str,
    body: &str,
    sound: bool,
) -> notify_rust::Notification {
    prepare_native_app(identifier);
    let mut notification = notify_rust::Notification::new();
    notification.summary(title);
    notification.body(body);
    notification.auto_icon();
    if sound {
        notification.sound_name(DEFAULT_SOUND);
    }
    #[cfg(windows)]
    apply_aumid(&mut notification, identifier);
    notification
}

fn prepare_native_app(identifier: &str) {
    #[cfg(target_os = "macos")]
    {
        let _ = notify_rust::set_application(if tauri::is_dev() {
            "com.apple.Terminal"
        } else {
            identifier
        });
    }
    let _ = identifier;
}

/// Installed app only — same guard as tauri-plugin-notification.
#[cfg(windows)]
fn apply_aumid(notification: &mut notify_rust::Notification, identifier: &str) {
    use std::path::MAIN_SEPARATOR as SEP;
    let Ok(exe) = tauri::utils::platform::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else {
        return;
    };
    let curr = dir.display().to_string();
    if curr.ends_with(format!("{SEP}target{SEP}debug").as_str())
        || curr.ends_with(format!("{SEP}target{SEP}release").as_str())
    {
        return;
    }
    notification.app_id(identifier);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::Notification;

    fn down(id: &str) -> Notification {
        Notification::Down {
            service_id: id.into(),
            title: id.into(),
            body: "HTTP 502 · 1.4s".into(),
        }
    }

    #[test]
    fn last_id_is_service_for_single_and_none_for_digest() {
        assert_eq!(last_notified_service_id(&down("pay")), Some("pay".into()));
        assert_eq!(
            last_notified_service_id(&Notification::recovered("pay", "Payments", 4_000)),
            Some("pay".into())
        );
        let digest = Notification::digest(&[("a", "API"), ("b", "Worker")]);
        assert_eq!(last_notified_service_id(&digest), None);
    }

    #[test]
    fn focus_launch_arg() {
        assert_eq!(parse_focus_args(&["pulse"]), None);
        assert_eq!(parse_focus_args(&["app", "pulse:focus"]), Some(None));
        assert_eq!(
            parse_focus_args(&["pulse:focus?id=abc"]),
            Some(Some("abc".into()))
        );
        assert_eq!(
            parse_focus_args(&["pulse://focus?id=abc"]),
            Some(Some("abc".into()))
        );
        assert_eq!(parse_focus_args(&["pulse:focus?id="]), Some(None));
    }

    #[test]
    fn permission_only_on_first_notify_save() {
        let asked = AtomicBool::new(false);
        assert!(!should_request_permission(false, &asked));
        assert!(!asked.load(Ordering::SeqCst));
        assert!(should_request_permission(true, &asked));
        assert!(!should_request_permission(true, &asked));
    }

    #[test]
    fn windows_default_body_click_is_activate() {
        use notify_rust::{CloseReason, NotificationResponse};
        assert!(is_click_response(&NotificationResponse::Default));
        assert!(is_click_response(&NotificationResponse::Action(
            "open".into()
        )));
        assert!(!is_click_response(&NotificationResponse::Closed(
            CloseReason::Dismissed
        )));
        assert!(!is_click_response(&NotificationResponse::Closed(
            CloseReason::Expired
        )));
    }
}
