//! macOS: let Pulse windows appear over a native fullscreen app.
//!
//! A normal NSWindow stays on the desktop Space. Activating it from a
//! fullscreen app switches Spaces (empty desktop) and greys the menu extra.
//! `FullScreenAuxiliary` + `CanJoinAllSpaces` keeps the window in the
//! current Space, including another app's fullscreen tile.

use tauri::WebviewWindow;

pub fn allow_over_fullscreen<R: tauri::Runtime>(window: &WebviewWindow<R>) {
    #[cfg(target_os = "macos")]
    macos::apply(window);
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

pub fn order_front<R: tauri::Runtime>(window: &WebviewWindow<R>) {
    #[cfg(target_os = "macos")]
    macos::order_front(window);
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSColor, NSStatusWindowLevel, NSWindow, NSWindowCollectionBehavior,
    };
    use tauri::WebviewWindow;
    use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};

    pub fn apply<R: tauri::Runtime>(window: &WebviewWindow<R>) {
        if MainThreadMarker::new().is_some() {
            apply_now(window);
            return;
        }
        let window = window.clone();
        let _ = window
            .clone()
            .run_on_main_thread(move || apply_now(&window));
    }

    fn apply_now<R: tauri::Runtime>(window: &WebviewWindow<R>) {
        let Ok(ptr) = window.ns_window() else {
            return;
        };
        if ptr.is_null() {
            return;
        }
        // SAFETY: Tauri's ns_window() is the AppKit NSWindow for this webview.
        // Called on the main thread; we only set collection behavior / level.
        let ns_window = unsafe { &*ptr.cast::<NSWindow>() };
        // Only the tray popover follows the user across Spaces / fullscreen.
        // Detail, settings, and editor stay on the Space where they were opened.
        if window.label() != "popover" {
            return;
        }
        let behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
            .union(NSWindowCollectionBehavior::FullScreenAuxiliary)
            .union(NSWindowCollectionBehavior::FullScreenDisallowsTiling)
            .union(NSWindowCollectionBehavior::CanJoinAllApplications)
            .union(NSWindowCollectionBehavior::Transient)
            .union(NSWindowCollectionBehavior::IgnoresCycle);
        ns_window.setLevel(NSStatusWindowLevel);
        ns_window.setOpaque(false);
        ns_window.setBackgroundColor(Some(&NSColor::clearColor()));
        let _ = window.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
        let _ = apply_vibrancy(
            window,
            NSVisualEffectMaterial::Popover,
            Some(NSVisualEffectState::Active),
            Some(12.0),
        );
        ns_window.setCollectionBehavior(behavior);
        ns_window.setHidesOnDeactivate(false);
        let _ = window.set_visible_on_all_workspaces(true);
    }

    pub fn order_front<R: tauri::Runtime>(window: &WebviewWindow<R>) {
        let show = {
            let window = window.clone();
            move || {
                let Ok(ptr) = window.ns_window() else {
                    return;
                };
                if ptr.is_null() {
                    return;
                }
                let ns_window = unsafe { &*ptr.cast::<NSWindow>() };
                ns_window.orderFrontRegardless();
            }
        };
        if MainThreadMarker::new().is_some() {
            show();
        } else {
            let _ = window.run_on_main_thread(show);
        }
    }
}
