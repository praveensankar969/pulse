//! macOS banners via `UNUserNotificationCenter`.
//!
//! `notify-rust` talks to deprecated `NSUserNotificationCenter`, which macOS 26
//! drops. Authorization was already UN; delivery must be UN too. Accessory
//! apps are treated as foreground, so a delegate must opt in to banners.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, MainThreadOnly};
use objc2_foundation::{MainThreadMarker, NSError, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::{
    UNMutableNotificationContent, UNNotification, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate, UNNotificationDismissActionIdentifier,
};

static DELEGATE_INSTALLED: AtomicBool = AtomicBool::new(false);
static ON_CLICK: Mutex<Option<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(None);

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "PulseUNDelegate"]
    #[ivars = ()]
    struct PulseUNDelegate;

    unsafe impl NSObjectProtocol for PulseUNDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for PulseUNDelegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion_handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion_handler.call((
                UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Sound,
            ));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion_handler: &block2::DynBlock<dyn Fn()>,
        ) {
            let action = response.actionIdentifier();
            let dismissed = unsafe { UNNotificationDismissActionIdentifier.isEqualToString(&action) };
            if !dismissed {
                if let Ok(guard) = ON_CLICK.lock() {
                    if let Some(on_click) = guard.as_ref() {
                        on_click();
                    }
                }
            }
            completion_handler.call(());
        }
    }
);

impl PulseUNDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

pub fn set_on_click(handler: Arc<dyn Fn() + Send + Sync>) {
    *ON_CLICK.lock().expect("notify click lock") = Some(handler);
}

/// Install a delegate so banners show while Pulse is an accessory (no Dock).
/// Returns whether a delegate is installed (this call or an earlier one).
pub fn install_delegate() -> bool {
    if DELEGATE_INSTALLED.load(Ordering::SeqCst) {
        return true;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        tracing::warn!("notification delegate must be installed on the main thread");
        return false;
    };
    let delegate = PulseUNDelegate::new(mtm);
    let center = UNUserNotificationCenter::currentNotificationCenter();
    center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    std::mem::forget(delegate);
    DELEGATE_INSTALLED.store(true, Ordering::SeqCst);
    true
}

pub fn open_system_notification_settings(bundle_id: &str) {
    let url = format!(
        "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id={bundle_id}"
    );
    if let Err(error) = std::process::Command::new("open").arg(&url).spawn() {
        tracing::warn!(error = %error, "could not open System Settings → Notifications");
    }
}

fn applescript_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Unsigned / ad-hoc Pulse.app is not allowed to use UNUserNotificationCenter
/// (signature id ≠ Info.plist, plist not bound). osascript still shows a banner.
fn fallback_osascript(title: &str, body: &str, sound: bool) {
    let mut src = format!(
        r#"display notification "{}" with title "Pulse" subtitle "{}""#,
        applescript_escape(body),
        applescript_escape(title),
    );
    if sound {
        src.push_str(r#" sound name "Glass""#);
    }
    match std::process::Command::new("osascript").arg("-e").arg(&src).status() {
        Ok(status) if status.success() => {
            tracing::info!(event = "notify_delivered", via = "osascript", "posted fallback banner");
        }
        Ok(status) => tracing::error!(code = ?status.code(), "osascript notification failed"),
        Err(error) => tracing::error!(error = %error, "osascript notification failed"),
    }
}

pub fn post(title: &str, body: &str, sound: bool) {
    if !install_delegate() {
        tracing::warn!("UN delegate missing; using osascript fallback");
        fallback_osascript(title, body, sound);
        return;
    }
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    if sound {
        content.setSound(Some(&UNNotificationSound::defaultSound()));
    }
    let id = NSString::from_str(&format!("pulse-{}", ulid::Ulid::new()));
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&id, &content, None);
    let center = UNUserNotificationCenter::currentNotificationCenter();
    let title_s = title.to_string();
    let body_s = body.to_string();
    let block = RcBlock::new(move |error: *mut NSError| {
        if error.is_null() {
            tracing::info!(event = "notify_delivered", via = "un", "UNUserNotificationCenter accepted toast");
            return;
        }
        let err = unsafe { &*error };
        tracing::error!(error = %err, "UNUserNotificationCenter rejected toast");
        fallback_osascript(&title_s, &body_s, sound);
    });
    center.addNotificationRequest_withCompletionHandler(&request, Some(&block));
}
