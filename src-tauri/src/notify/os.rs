//! OS toasts via `tauri-plugin-notification`.
//!
//! Click is best-effort: show the popover (macOS re-assert accessory; Windows
//! honors `pulse:focus?id=` / AUMID when the plugin set one). Plugin "actions"
//! are mobile-only. No quiet-hours flush.

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

        let mut builder = self.app.notification().builder().title(title).body(body);
        if self.sound_enabled() {
            builder = builder.sound(DEFAULT_SOUND);
        }
        if let Err(error) = builder.show() {
            tracing::warn!(error = %error, "os toast failed");
        }
    }
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
}
