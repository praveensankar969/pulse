//! OS resume hooks. Overdue-interval detection is the portable fallback.

/// Listeners last for process lifetime; dropping the guard does not unregister.
#[derive(Default)]
pub struct WakeGuard {
    _keep: (),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    Sleep,
    Wake,
}

/// Keep the returned guard alive for as long as notifications should fire.
pub fn listen<F>(on_event: F) -> WakeGuard
where
    F: Fn(PowerEvent) + Send + Sync + 'static,
{
    listen_impl(on_event);
    WakeGuard { _keep: () }
}

#[cfg(target_os = "macos")]
fn listen_impl<F>(on_event: F)
where
    F: Fn(PowerEvent) + Send + Sync + 'static,
{
    macos::register(on_event);
}

#[cfg(target_os = "windows")]
fn listen_impl<F>(on_event: F)
where
    F: Fn(PowerEvent) + Send + Sync + 'static,
{
    windows::register(on_event);
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn listen_impl<F>(_on_event: F)
where
    F: Fn(PowerEvent) + Send + Sync + 'static,
{
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ptr::NonNull;
    use std::sync::Arc;

    use block2::RcBlock;
    use objc2_app_kit::{
        NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceWillSleepNotification,
    };
    use objc2_foundation::{NSNotification, NSNotificationCenter, NSNotificationName};

    use super::PowerEvent;

    pub fn register<F>(on_event: F)
    where
        F: Fn(PowerEvent) + Send + Sync + 'static,
    {
        let on_event = Arc::new(on_event);
        let workspace = NSWorkspace::sharedWorkspace();
        let center = workspace.notificationCenter();

        // Retained observers must outlive the process; leak them.
        std::mem::forget(add(&center, unsafe { NSWorkspaceWillSleepNotification }, {
            let on_event = Arc::clone(&on_event);
            move || on_event(PowerEvent::Sleep)
        }));
        std::mem::forget(add(&center, unsafe { NSWorkspaceDidWakeNotification }, {
            let on_event = Arc::clone(&on_event);
            move || on_event(PowerEvent::Wake)
        }));
        std::mem::forget(center);
    }

    fn add(
        center: &NSNotificationCenter,
        name: &NSNotificationName,
        on_event: impl Fn() + Send + Sync + 'static,
    ) -> objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_foundation::NSObjectProtocol>>
    {
        let block = RcBlock::new(move |_notification: NonNull<NSNotification>| {
            on_event();
        });
        // Workspace notifications arrive on the registering thread (app setup).
        unsafe { center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block) }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::sync::Arc;
    use std::thread;

    use super::PowerEvent;

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, GetWindowLongPtrW,
        RegisterClassW, SetWindowLongPtrW, TranslateMessage, CREATESTRUCTW, CW_USEDEFAULT,
        GWLP_USERDATA, HWND_MESSAGE, MSG, WM_CREATE, WM_DESTROY, WM_NCCREATE, WM_POWERBROADCAST,
        WNDCLASSW, WS_OVERLAPPED,
    };

    const PBT_APMSUSPEND: usize = 0x0004;
    const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;

    struct State {
        on_event: Arc<dyn Fn(PowerEvent) + Send + Sync>,
    }

    pub fn register<F>(on_event: F)
    where
        F: Fn(PowerEvent) + Send + Sync + 'static,
    {
        let state = Box::new(State {
            on_event: Arc::new(on_event),
        });
        thread::spawn(move || unsafe {
            run_message_window(state);
        });
    }

    unsafe fn run_message_window(state: Box<State>) {
        let class_name: Vec<u16> = "PulseWakeWnd\0".encode_utf16().collect();
        let class = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(std::ptr::null()),
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            WS_OVERLAPPED,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            Box::into_raw(state).cast(),
        );
        if hwnd.is_null() {
            return;
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_NCCREATE || msg == WM_CREATE {
            let create = lparam as *const CREATESTRUCTW;
            if !create.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*create).lpCreateParams as isize);
            }
        }
        if msg == WM_POWERBROADCAST {
            let userdata = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if userdata != 0 {
                let state = &*(userdata as *const State);
                match wparam {
                    PBT_APMSUSPEND => (state.on_event)(PowerEvent::Sleep),
                    PBT_APMRESUMEAUTOMATIC => (state.on_event)(PowerEvent::Wake),
                    _ => {}
                }
            }
            return 1;
        }
        if msg == WM_DESTROY {
            let userdata = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if userdata != 0 {
                drop(Box::from_raw(userdata as *mut State));
            }
            windows_sys::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
            return 0;
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_returns_a_guard() {
        let _guard = listen(|_| {});
    }
}
