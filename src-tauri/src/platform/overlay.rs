//! macOS: keep Pulse on the Space the user is actually looking at.
//!
//! A normal NSWindow lives on the desktop Space. Becoming Regular + showing
//! it from a fullscreen app switches Spaces (empty desktop) and leaves Pulse
//! in a state where the tray popover dies on the next hover.
//!
//! The popover joins every Space (menu extra). Editor / detail / settings
//! appear on the *active* Space, including over fullscreen, then stay there.

use tauri::WebviewWindow;

pub fn allow_over_fullscreen<R: tauri::Runtime>(window: &WebviewWindow<R>) {
    #[cfg(target_os = "macos")]
    macos::apply_popover(window);
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

/// Editor / detail / settings: current Space only, including fullscreen.
/// Collection behavior is applied on the same main-thread turn as `show`, so
/// AppKit never parks the window on the desktop Space first.
pub fn present_on_active_space<R: tauri::Runtime>(window: &WebviewWindow<R>) {
    #[cfg(target_os = "macos")]
    macos::present_utility(window);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn order_front<R: tauri::Runtime>(window: &WebviewWindow<R>) {
    #[cfg(target_os = "macos")]
    macos::order_front(window);
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

/// True when the cursor is over this window's frame (screen coordinates).
pub fn cursor_inside_window<R: tauri::Runtime>(window: &WebviewWindow<R>) -> bool {
    #[cfg(target_os = "macos")]
    {
        return macos::cursor_inside(window);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let Ok(cursor) = window.cursor_position() else {
            return false;
        };
        let Ok(pos) = window.outer_position() else {
            return false;
        };
        let Ok(size) = window.outer_size() else {
            return false;
        };
        let x = cursor.x as i32;
        let y = cursor.y as i32;
        x >= pos.x
            && y >= pos.y
            && x < pos.x + size.width as i32
            && y < pos.y + size.height as i32
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSColor, NSStatusWindowLevel, NSWindow, NSWindowCollectionBehavior,
    };
    use tauri::WebviewWindow;
    use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};

    pub fn apply_popover<R: tauri::Runtime>(window: &WebviewWindow<R>) {
        on_main(window, apply_popover_now);
    }

    pub fn present_utility<R: tauri::Runtime>(window: &WebviewWindow<R>) {
        on_main(window, |window| {
            apply_utility_now(window);
            let _ = window.unminimize();
            let _ = window.show();
            if let Some(ns_window) = ns_window(window) {
                ns_window.orderFrontRegardless();
            }
            let _ = window.set_focus();
        });
    }

    fn on_main<R, F>(window: &WebviewWindow<R>, apply: F)
    where
        R: tauri::Runtime,
        F: FnOnce(&WebviewWindow<R>) + Send + 'static,
    {
        if MainThreadMarker::new().is_some() {
            apply(window);
            return;
        }
        let window = window.clone();
        let _ = window.clone().run_on_main_thread(move || apply(&window));
    }

    fn ns_window<R: tauri::Runtime>(window: &WebviewWindow<R>) -> Option<&NSWindow> {
        let ptr = window.ns_window().ok()?;
        if ptr.is_null() {
            return None;
        }
        // SAFETY: Tauri's ns_window() is the AppKit NSWindow for this webview.
        Some(unsafe { &*ptr.cast::<NSWindow>() })
    }

    fn apply_popover_now<R: tauri::Runtime>(window: &WebviewWindow<R>) {
        let Some(ns_window) = ns_window(window) else {
            return;
        };
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

    fn apply_utility_now<R: tauri::Runtime>(window: &WebviewWindow<R>) {
        let Some(ns_window) = ns_window(window) else {
            return;
        };
        // Active Space only — not CanJoinAllSpaces, so the form does not follow
        // the user after they leave. FullScreenAuxiliary avoids a Space switch.
        let behavior = NSWindowCollectionBehavior::MoveToActiveSpace
            .union(NSWindowCollectionBehavior::FullScreenAuxiliary)
            .union(NSWindowCollectionBehavior::FullScreenDisallowsTiling)
            .union(NSWindowCollectionBehavior::IgnoresCycle);
        ns_window.setLevel(objc2_app_kit::NSFloatingWindowLevel);
        ns_window.setCollectionBehavior(behavior);
        ns_window.setHidesOnDeactivate(false);
        let _ = window.set_visible_on_all_workspaces(false);
    }

    pub fn cursor_inside<R: tauri::Runtime>(window: &WebviewWindow<R>) -> bool {
        let Some(ns_window) = ns_window(window) else {
            return false;
        };
        let loc = objc2_app_kit::NSEvent::mouseLocation();
        let frame = ns_window.frame();
        loc.x >= frame.origin.x
            && loc.y >= frame.origin.y
            && loc.x < frame.origin.x + frame.size.width
            && loc.y < frame.origin.y + frame.size.height
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
