//! Color (non-template) tray mark, left-click blur protocol, native right-click menu.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::image::Image;
use tauri::menu::{MenuBuilder, MenuEvent};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Listener, LogicalPosition, Manager, PhysicalPosition, Position, Size, State,
    WebviewWindow,
};

use crate::domain::{ServiceView, UiState};
use crate::ipc::AppState;

pub const SUPPRESS_BLUR_MS: u64 = 250;
pub const WORK_AREA_INSET: i32 = 12;

const OK: [u8; 3] = [0x3d, 0xdc, 0x97];
const WARN: [u8; 3] = [0xf5, 0xb9, 0x42];
const DANGER: [u8; 3] = [0xf0, 0x53, 0x4a];
const MUTED: [u8; 3] = [0x6b, 0x73, 0x80];
const SLASH_OFFLINE: [u8; 3] = [0xc8, 0xcd, 0xd6];
const WHITE: [u8; 3] = [0xff, 0xff, 0xff];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayMark {
    Healthy,
    Degraded,
    Down { count: u32 },
    Hollow,
    Offline,
    PollerDead,
}

impl TrayMark {
    pub fn tooltip(self) -> &'static str {
        match self {
            Self::Healthy => "Pulse · all healthy",
            Self::Degraded => "Pulse · degraded",
            Self::Down { .. } => "Pulse · services down",
            Self::Hollow => "Pulse",
            Self::Offline => "Pulse · offline",
            Self::PollerDead => "Pulse · checker stopped",
        }
    }
}

pub struct TraySnapshot<'a> {
    pub services: &'a [ServiceView],
    pub offline: bool,
    pub poller_dead: bool,
}

/// Worst-of across unpaused non-pending services. Snooze does not change the mark.
pub fn mark_from(snap: TraySnapshot<'_>) -> TrayMark {
    if snap.poller_dead {
        return TrayMark::PollerDead;
    }
    if snap.offline {
        return TrayMark::Offline;
    }

    let active: Vec<&ServiceView> = snap
        .services
        .iter()
        .filter(|view| !view.service.paused && view.state != UiState::Paused)
        .collect();
    if active.is_empty() || active.iter().all(|view| view.state == UiState::Pending) {
        return TrayMark::Hollow;
    }

    let down = active
        .iter()
        .filter(|view| view.state == UiState::Down)
        .count() as u32;
    if down > 0 {
        return TrayMark::Down { count: down };
    }
    if active.iter().any(|view| view.state == UiState::Degraded) {
        return TrayMark::Degraded;
    }
    TrayMark::Healthy
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickButton {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickOutcome {
    None,
    Toggle,
    ShowOnly,
}

/// Mouse-down suppress-blur / mouse-up toggle. Right-click menu dismiss skips toggle.
#[derive(Debug, Clone, Default)]
pub struct ClickProtocol {
    suppress_until: Option<Instant>,
    menu_open: bool,
    skip_toggle: bool,
}

impl ClickProtocol {
    pub fn on_down(&mut self, button: ClickButton, now: Instant) {
        let showing = self.menu_showing(now);
        self.suppress_until = Some(now + Duration::from_millis(SUPPRESS_BLUR_MS));
        match button {
            ClickButton::Right => {
                self.menu_open = true;
                self.skip_toggle = false;
            }
            ClickButton::Left => {
                // Only the dismiss click inside the prior suppress window skips toggle.
                self.skip_toggle = showing;
                self.menu_open = false;
            }
        }
    }

    pub fn on_up(&mut self, button: ClickButton, overflow: bool) -> ClickOutcome {
        if button != ClickButton::Left {
            return ClickOutcome::None;
        }
        if self.skip_toggle {
            self.skip_toggle = false;
            return ClickOutcome::None;
        }
        if overflow {
            ClickOutcome::ShowOnly
        } else {
            ClickOutcome::Toggle
        }
    }

    pub fn on_menu_closed(&mut self) {
        self.menu_open = false;
    }

    pub fn should_suppress_blur(&self, now: Instant) -> bool {
        self.suppress_until.is_some_and(|until| now < until)
    }

    fn menu_showing(&self, now: Instant) -> bool {
        self.menu_open && self.should_suppress_blur(now)
    }
}

#[derive(Clone)]
pub struct TrayHandle {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    protocol: ClickProtocol,
    offline: bool,
    poller_dead: bool,
    views: Vec<ServiceView>,
    last_rect: Option<tauri::Rect>,
    apply_icon: Option<Arc<dyn Fn(TrayMark) + Send + Sync>>,
    query_rect: Option<Arc<dyn Fn() -> Option<tauri::Rect> + Send + Sync>>,
    first_run_until_focus: bool,
}

impl Default for TrayHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl TrayHandle {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                protocol: ClickProtocol::default(),
                offline: false,
                poller_dead: false,
                views: Vec::new(),
                last_rect: None,
                apply_icon: None,
                query_rect: None,
                first_run_until_focus: false,
            })),
        }
    }

    pub fn poller_dead_hook(&self) -> Arc<dyn Fn(bool) + Send + Sync> {
        let handle = self.clone();
        Arc::new(move |dead| handle.set_poller_dead(dead))
    }

    pub fn set_poller_dead(&self, dead: bool) {
        self.commit_paint(|inner| inner.poller_dead = dead);
    }

    pub fn set_offline(&self, offline: bool) {
        self.commit_paint(|inner| inner.offline = offline);
    }

    pub fn apply_services(&self, views: &[ServiceView]) {
        self.commit_paint(|inner| inner.views = views.to_vec());
    }

    pub fn mark(&self) -> TrayMark {
        self.inner.lock().expect("tray lock").mark()
    }

    pub fn should_suppress_blur(&self) -> bool {
        let inner = self.inner.lock().expect("tray lock");
        inner.first_run_until_focus || inner.protocol.should_suppress_blur(Instant::now())
    }

    pub fn arm_first_run_show(&self) {
        let mut inner = self.inner.lock().expect("tray lock");
        inner.first_run_until_focus = true;
        inner.protocol.on_down(ClickButton::Left, Instant::now());
    }

    pub fn note_popover_focused(&self) {
        self.inner.lock().expect("tray lock").first_run_until_focus = false;
    }

    pub fn remember_rect(&self, rect: &tauri::Rect) {
        if rect_is_empty(rect) {
            return;
        }
        self.inner.lock().expect("tray lock").last_rect = Some(*rect);
    }

    pub fn last_rect(&self) -> Option<tauri::Rect> {
        self.inner.lock().expect("tray lock").last_rect
    }

    pub fn query_icon_rect(&self) -> Option<tauri::Rect> {
        let query = self.inner.lock().expect("tray lock").query_rect.clone();
        query.and_then(|query| query())
    }

    fn bind_icon(&self, apply: Arc<dyn Fn(TrayMark) + Send + Sync>) {
        self.commit_paint(|inner| inner.apply_icon = Some(apply));
    }

    fn bind_rect_query(&self, query: Arc<dyn Fn() -> Option<tauri::Rect> + Send + Sync>) {
        self.inner.lock().expect("tray lock").query_rect = Some(query);
    }

    /// Snapshot mark under the lock, then paint after drop. `set_icon` blocks on main.
    fn commit_paint(&self, update: impl FnOnce(&mut Inner)) {
        let (mark, apply) = {
            let mut inner = self.inner.lock().expect("tray lock");
            update(&mut inner);
            (inner.mark(), inner.apply_icon.clone())
        };
        if let Some(apply) = apply {
            apply(mark);
        }
    }

    fn on_down(&self, button: ClickButton) {
        self.inner
            .lock()
            .expect("tray lock")
            .protocol
            .on_down(button, Instant::now());
    }

    fn on_up(&self, button: ClickButton, overflow: bool) -> ClickOutcome {
        self.inner
            .lock()
            .expect("tray lock")
            .protocol
            .on_up(button, overflow)
    }

    fn on_menu_closed(&self) {
        self.inner
            .lock()
            .expect("tray lock")
            .protocol
            .on_menu_closed();
    }
}

impl Inner {
    fn mark(&self) -> TrayMark {
        mark_from(TraySnapshot {
            services: &self.views,
            offline: self.offline,
            poller_dead: self.poller_dead,
        })
    }
}

#[tauri::command(rename_all = "camelCase")]
pub fn should_suppress_blur(tray: State<TrayHandle>) -> bool {
    tray.should_suppress_blur()
}

pub fn install<R: tauri::Runtime>(app: &AppHandle<R>, tray: TrayHandle) -> tauri::Result<()> {
    let menu = MenuBuilder::new(app)
        .text("check-all", "Check all")
        .text("settings", "Settings")
        .text("quit", "Quit")
        .build()?;

    let icon_size = paint_size();
    let rgba = paint_mark(TrayMark::Hollow, icon_size, logical_1x());
    let image = Image::new_owned(rgba, icon_size, icon_size);

    let event_tray = tray.clone();
    let menu_tray = tray.clone();
    let built = TrayIconBuilder::with_id("pulse")
        .icon(image)
        .icon_as_template(false)
        .tooltip("Pulse")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| {
            menu_tray.on_menu_closed();
            handle_menu(app, &event);
        })
        .on_tray_icon_event(move |icon, event| {
            handle_tray_event(icon.app_handle(), &event_tray, event);
        })
        .build(app)?;

    // Status color must survive macOS menu-bar recolor.
    let _ = built.set_icon_as_template(false);

    if let Ok(Some(rect)) = built.rect() {
        tray.remember_rect(&rect);
    }
    let query_icon = built.clone();
    tray.bind_rect_query(Arc::new(move || query_icon.rect().ok().flatten()));

    #[cfg(windows)]
    {
        let hook_tray = tray.clone();
        let hook_app = app.clone();
        if let Ok(hwnd) = built.with_inner_tray_icon(|inner| inner.window_handle() as isize) {
            overflow_hook::install(hwnd, move |button, down| {
                handle_getrect_fail(&hook_app, &hook_tray, button, down);
            });
        }
    }

    let apply_icon = {
        let icon = built.clone();
        Arc::new(move |mark: TrayMark| {
            let size = paint_size();
            let rgba = paint_mark(mark, size, logical_1x());
            let _ = icon.set_icon(Some(Image::new_owned(rgba, size, size)));
            let _ = icon.set_tooltip(Some(mark.tooltip()));
            let _ = icon.set_icon_as_template(false);
        }) as Arc<dyn Fn(TrayMark) + Send + Sync>
    };
    tray.bind_icon(apply_icon);

    let services_tray = tray.clone();
    let _ = app.listen("pulse://services", move |event| {
        if let Ok(views) = serde_json::from_str::<Vec<ServiceView>>(event.payload()) {
            services_tray.apply_services(&views);
        }
    });

    let offline_tray = tray.clone();
    let _ = app.listen("pulse://offline", move |event| {
        if let Ok(payload) = serde_json::from_str::<OfflinePayload>(event.payload()) {
            offline_tray.set_offline(payload.offline);
        }
    });

    if let Some(popover) = app.get_webview_window("popover") {
        let blur_tray = tray.clone();
        let hide = popover.clone();
        popover.on_window_event(move |event| match event {
            tauri::WindowEvent::Focused(true) => blur_tray.note_popover_focused(),
            tauri::WindowEvent::Focused(false) if !blur_tray.should_suppress_blur() => {
                let _ = hide.hide();
            }
            _ => {}
        });
    }

    Ok(())
}

#[derive(serde::Deserialize)]
struct OfflinePayload {
    offline: bool,
}

fn handle_menu<R: tauri::Runtime>(app: &AppHandle<R>, event: &MenuEvent) {
    match event.id().as_ref() {
        "check-all" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Some(state) = app.try_state::<AppState>() {
                    state.scheduler.check_all().await;
                }
            });
        }
        "settings" => crate::platform::settings::open_settings(app),
        "quit" => app.exit(0),
        _ => {}
    }
}

fn handle_tray_event<R: tauri::Runtime>(
    app: &AppHandle<R>,
    tray: &TrayHandle,
    event: TrayIconEvent,
) {
    let TrayIconEvent::Click {
        button,
        button_state,
        rect,
        ..
    } = event
    else {
        return;
    };
    let click = match button {
        MouseButton::Left => ClickButton::Left,
        MouseButton::Right => ClickButton::Right,
        MouseButton::Middle => return,
    };
    match button_state {
        MouseButtonState::Down => {
            if !click_is_overflow(app, &rect) {
                tray.remember_rect(&rect);
            }
            tray.on_down(click);
        }
        MouseButtonState::Up => {
            let overflow = click_is_overflow(app, &rect);
            if !overflow {
                tray.remember_rect(&rect);
            }
            match tray.on_up(click, overflow) {
                ClickOutcome::Toggle => apply_visibility(app, Some(&rect), false),
                ClickOutcome::ShowOnly => apply_visibility(app, None, true),
                ClickOutcome::None => {}
            }
        }
    }
}

#[cfg(windows)]
fn handle_getrect_fail<R: tauri::Runtime>(
    app: &AppHandle<R>,
    tray: &TrayHandle,
    button: ClickButton,
    down: bool,
) {
    if down {
        tray.on_down(button);
        return;
    }
    match tray.on_up(button, true) {
        ClickOutcome::ShowOnly | ClickOutcome::Toggle => apply_visibility(app, None, true),
        ClickOutcome::None => {}
    }
}

pub fn rect_is_empty(rect: &tauri::Rect) -> bool {
    match rect.size {
        Size::Physical(size) => size.width < 1 || size.height < 1,
        Size::Logical(size) => size.width < 1.0 || size.height < 1.0,
    }
}

/// Icon sits on the taskbar / menu bar when it is not fully inside the work area.
pub fn icon_on_taskbar(icon: WorkArea, work: WorkArea) -> bool {
    let (ix, iy, iw, ih) = icon;
    let (wx, wy, ww, wh) = work;
    let ir = ix.saturating_add_unsigned(iw);
    let ib = iy.saturating_add_unsigned(ih);
    let wr = wx.saturating_add_unsigned(ww);
    let wb = wy.saturating_add_unsigned(wh);
    !(ix >= wx && iy >= wy && ir <= wr && ib <= wb)
}

fn rect_box(rect: &tauri::Rect) -> WorkArea {
    let (x, y) = match rect.position {
        Position::Physical(pos) => (pos.x, pos.y),
        Position::Logical(pos) => (pos.x as i32, pos.y as i32),
    };
    let (w, h) = match rect.size {
        Size::Physical(size) => (size.width, size.height),
        Size::Logical(size) => (size.width as u32, size.height as u32),
    };
    (x, y, w, h)
}

fn click_is_overflow<R: tauri::Runtime>(app: &AppHandle<R>, rect: &tauri::Rect) -> bool {
    if rect_is_empty(rect) {
        return true;
    }
    let Some(window) = app.get_webview_window("popover") else {
        return false;
    };
    let icon = rect_box(rect);
    let cx = icon.0 + icon.2 as i32 / 2;
    let cy = icon.1 + icon.3 as i32 / 2;
    match monitor_work_area_for_point(&window, cx, cy) {
        Some(area) => !icon_on_taskbar(icon, area),
        None => false,
    }
}

/// Show-only + work-area. Not gated on a delivered Click (GetRect-fail / overflow).
pub fn show_popover_if_hidden<R: tauri::Runtime>(app: &AppHandle<R>) {
    apply_visibility(app, None, true);
}

/// First-run: place under the tray icon when we have a rect; suppress blur until focused.
pub fn show_first_run<R: tauri::Runtime>(app: &AppHandle<R>) -> bool {
    let Some(tray) = app.try_state::<TrayHandle>() else {
        return false;
    };
    tray.arm_first_run_show();
    let rect = tray.query_icon_rect().or_else(|| tray.last_rect());
    let usable = rect
        .as_ref()
        .filter(|rect| !rect_is_empty(rect) && !click_is_overflow(app, rect));
    apply_visibility(app, usable, true);
    app.get_webview_window("popover")
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false)
}

/// Global hotkey: same placement as a left click when we have a tray rect.
pub fn toggle_popover<R: tauri::Runtime>(app: &AppHandle<R>) {
    let rect = app
        .try_state::<TrayHandle>()
        .and_then(|tray| tray.last_rect());
    apply_visibility(app, rect.as_ref(), false);
}

fn apply_visibility<R: tauri::Runtime>(
    app: &AppHandle<R>,
    rect: Option<&tauri::Rect>,
    show_only: bool,
) {
    let Some(window) = app.get_webview_window("popover") else {
        return;
    };
    let visible = window.is_visible().unwrap_or(false);
    if show_only {
        if !visible {
            match rect {
                Some(rect) if !rect_is_empty(rect) => place_popover(&window, rect),
                _ => place_work_area_fallback(&window),
            }
            let _ = window.show();
            let _ = window.set_focus();
        }
        return;
    }
    if visible {
        let _ = window.hide();
        return;
    }
    match rect {
        Some(rect) if !rect_is_empty(rect) => place_popover(&window, rect),
        _ => place_work_area_fallback(&window),
    }
    let _ = window.show();
    let _ = window.set_focus();
}

fn place_popover<R: tauri::Runtime>(window: &WebviewWindow<R>, rect: &tauri::Rect) {
    let Ok(outer) = window.outer_size() else {
        return;
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    let (x, y) = match rect.position {
        Position::Physical(pos) => (pos.x as f64, pos.y as f64),
        Position::Logical(pos) => (pos.x * scale, pos.y * scale),
    };
    let (width, height) = match rect.size {
        Size::Physical(size) => (size.width as f64, size.height as f64),
        Size::Logical(size) => (size.width * scale, size.height * scale),
    };
    let left = x + width - f64::from(outer.width);
    let top = if cfg!(target_os = "macos") {
        y + height
    } else {
        y - f64::from(outer.height)
    };
    let cx = (x + width / 2.0) as i32;
    let cy = (y + height / 2.0) as i32;
    let (left, top) = match work_area_containing(window, cx, cy) {
        Some(area) => clamp_to_work_area(
            left as i32,
            top as i32,
            outer.width,
            outer.height,
            area,
            WORK_AREA_INSET,
        ),
        None => (left as i32, top as i32),
    };
    let _ = window.set_position(PhysicalPosition::new(left, top));
}

fn place_work_area_fallback<R: tauri::Runtime>(window: &WebviewWindow<R>) {
    let Ok(outer) = window.outer_size() else {
        return;
    };
    let Some(area) = fallback_work_area(window) else {
        let _ = window.set_position(LogicalPosition::new(
            f64::from(WORK_AREA_INSET),
            f64::from(WORK_AREA_INSET),
        ));
        return;
    };
    let from_top = cfg!(target_os = "macos");
    let (x, y) = work_area_anchor(area, outer.width, outer.height, WORK_AREA_INSET, from_top);
    let _ = window.set_position(PhysicalPosition::new(x, y));
}

pub type WorkArea = (i32, i32, u32, u32);

/// Overflow / first-run fallback: Windows bottom-right, macOS top-right (menu bar).
pub fn work_area_anchor(
    area: WorkArea,
    pop_w: u32,
    pop_h: u32,
    inset: i32,
    from_top: bool,
) -> (i32, i32) {
    let (area_x, area_y, area_w, area_h) = area;
    let x = area_x + area_w as i32 - pop_w as i32 - inset;
    let y = if from_top {
        area_y + inset
    } else {
        area_y + area_h as i32 - pop_h as i32 - inset
    };
    (x, y)
}

/// Keep the popover on the same monitor as the tray / cursor.
pub fn clamp_to_work_area(
    x: i32,
    y: i32,
    pop_w: u32,
    pop_h: u32,
    area: WorkArea,
    inset: i32,
) -> (i32, i32) {
    let (area_x, area_y, area_w, area_h) = area;
    let min_x = area_x + inset;
    let min_y = area_y + inset;
    let max_x = area_x + area_w as i32 - pop_w as i32 - inset;
    let max_y = area_y + area_h as i32 - pop_h as i32 - inset;
    (
        x.clamp(min_x, max_x.max(min_x)),
        y.clamp(min_y, max_y.max(min_y)),
    )
}

/// Which monitor rectangle contains `(px, py)`. Used for multi-monitor placement.
pub fn monitor_containing(monitors: &[WorkArea], px: i32, py: i32) -> Option<usize> {
    monitors
        .iter()
        .position(|&(x, y, w, h)| px >= x && py >= y && px < x + w as i32 && py < y + h as i32)
}

fn work_area_of(monitor: &tauri::Monitor) -> WorkArea {
    let area = monitor.work_area();
    (
        area.position.x,
        area.position.y,
        area.size.width,
        area.size.height,
    )
}

fn monitor_bounds(monitor: &tauri::Monitor) -> WorkArea {
    let pos = monitor.position();
    let size = monitor.size();
    (pos.x, pos.y, size.width, size.height)
}

fn work_area_containing<R: tauri::Runtime>(
    window: &WebviewWindow<R>,
    px: i32,
    py: i32,
) -> Option<WorkArea> {
    monitor_work_area_for_point(window, px, py)
        .filter(|(x, y, w, h)| px >= *x && py >= *y && px < *x + *w as i32 && py < *y + *h as i32)
}

fn monitor_work_area_for_point<R: tauri::Runtime>(
    window: &WebviewWindow<R>,
    px: i32,
    py: i32,
) -> Option<WorkArea> {
    let monitors = window.available_monitors().ok()?;
    let idx = monitor_containing(
        &monitors.iter().map(monitor_bounds).collect::<Vec<_>>(),
        px,
        py,
    )?;
    monitors.get(idx).map(work_area_of)
}

fn fallback_work_area<R: tauri::Runtime>(window: &WebviewWindow<R>) -> Option<WorkArea> {
    if let Ok(cursor) = window.cursor_position() {
        if let Some(area) = work_area_containing(window, cursor.x as i32, cursor.y as i32) {
            return Some(area);
        }
    }
    if let Some(tray) = window.app_handle().try_state::<TrayHandle>() {
        if let Some(rect) = tray.last_rect() {
            let scale = window.scale_factor().unwrap_or(1.0);
            let (x, y) = match rect.position {
                Position::Physical(pos) => (pos.x, pos.y),
                Position::Logical(pos) => ((pos.x * scale) as i32, (pos.y * scale) as i32),
            };
            if let Some(area) = work_area_containing(window, x, y) {
                return Some(area);
            }
        }
    }
    window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten())
        .map(|monitor| work_area_of(&monitor))
}

fn paint_size() -> u32 {
    if cfg!(target_os = "windows") {
        32
    } else {
        36
    }
}

fn logical_1x() -> f32 {
    if cfg!(target_os = "windows") {
        16.0
    } else {
        18.0
    }
}

/// Geometric circle. `logical_1x` is 18 (mac) or 16 (win).
pub fn paint_mark(mark: TrayMark, size: u32, logical_1x: f32) -> Vec<u8> {
    let mut px = vec![0u8; size as usize * size as usize * 4];
    let scale = size as f32 / logical_1x;
    let cx = size as f32 / 2.0;
    let cy = size as f32 / 2.0;
    let radius = 4.0 * scale;
    let stroke = 1.5 * scale;
    let slash_half = 6.0 * scale;
    let slash_w = 0.75 * scale;

    match mark {
        TrayMark::Healthy => fill_circle(&mut px, size, cx, cy, radius, OK),
        TrayMark::Degraded => fill_circle(&mut px, size, cx, cy, radius, WARN),
        TrayMark::Down { count } => {
            fill_circle(&mut px, size, cx, cy, radius, DANGER);
            if count > 0 {
                draw_badge(&mut px, size, scale, count);
            }
        }
        TrayMark::Hollow => ring_circle(&mut px, size, cx, cy, radius, stroke, MUTED),
        TrayMark::Offline => {
            fill_circle(&mut px, size, cx, cy, radius, MUTED);
            draw_slash(&mut px, size, cx, cy, slash_half, slash_w, SLASH_OFFLINE);
        }
        TrayMark::PollerDead => {
            ring_circle(&mut px, size, cx, cy, radius, stroke, DANGER);
            draw_slash(&mut px, size, cx, cy, slash_half, slash_w, DANGER);
        }
    }
    px
}

fn fill_circle(px: &mut [u8], size: u32, cx: f32, cy: f32, radius: f32, rgb: [u8; 3]) {
    for y in 0..size {
        for x in 0..size {
            let cover = circle_cover(x, y, cx, cy, radius);
            if cover > 0.0 {
                blend(pixel(px, size, x, y), rgb, cover);
            }
        }
    }
}

fn ring_circle(px: &mut [u8], size: u32, cx: f32, cy: f32, radius: f32, stroke: f32, rgb: [u8; 3]) {
    let inner = (radius - stroke).max(0.0);
    for y in 0..size {
        for x in 0..size {
            let outer = circle_cover(x, y, cx, cy, radius);
            let hole = circle_cover(x, y, cx, cy, inner);
            let cover = (outer - hole).clamp(0.0, 1.0);
            if cover > 0.0 {
                blend(pixel(px, size, x, y), rgb, cover);
            }
        }
    }
}

fn draw_slash(px: &mut [u8], size: u32, cx: f32, cy: f32, half: f32, half_w: f32, rgb: [u8; 3]) {
    let x1 = cx - half;
    let y1 = cy + half;
    let x2 = cx + half;
    let y2 = cy - half;
    for y in 0..size {
        for x in 0..size {
            let px_c = x as f32 + 0.5;
            let py_c = y as f32 + 0.5;
            let dist = dist_to_segment(px_c, py_c, x1, y1, x2, y2);
            let cover = (half_w + 0.5 - dist).clamp(0.0, 1.0);
            if cover > 0.0 {
                blend(pixel(px, size, x, y), rgb, cover);
            }
        }
    }
}

fn draw_badge(px: &mut [u8], size: u32, scale: f32, count: u32) {
    let label = badge_label(count);
    let cell = scale.max(1.0);
    let glyph_w = 3.0 * cell;
    let glyph_h = 5.0 * cell;
    let gap = cell;
    let digits = label.len() as f32;
    let text_w = digits * glyph_w + (digits - 1.0).max(0.0) * gap;
    let pad_x = 2.0 * scale;
    let pad_y = 1.5 * scale;
    let bw = (text_w + pad_x * 2.0).max(6.0 * scale);
    let bh = glyph_h + pad_y * 2.0;
    let bx = size as f32 - bw;
    let by = 0.0;
    let r = bh / 2.0;

    for y in 0..size {
        for x in 0..size {
            let cover = round_rect_cover(x, y, bx, by, bw, bh, r);
            if cover > 0.0 {
                blend(pixel(px, size, x, y), DANGER, cover);
            }
        }
    }

    let mut tx = bx + (bw - text_w) / 2.0;
    let ty = by + (bh - glyph_h) / 2.0;
    for ch in label.chars() {
        draw_glyph(px, size, tx, ty, cell, ch, WHITE);
        tx += glyph_w + gap;
    }
}

fn badge_label(count: u32) -> String {
    if count > 99 {
        "99+".into()
    } else {
        count.to_string()
    }
}

const GLYPHS: [(char, [u8; 15]); 11] = [
    ('0', [1, 1, 1, 1, 0, 1, 1, 0, 1, 1, 0, 1, 1, 1, 1]),
    ('1', [0, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 1, 1, 1]),
    ('2', [1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 0, 0, 1, 1, 1]),
    ('3', [1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1]),
    ('4', [1, 0, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 1]),
    ('5', [1, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 1]),
    ('6', [1, 1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 1, 1, 1, 1]),
    ('7', [1, 1, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1]),
    ('8', [1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1]),
    ('9', [1, 1, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1]),
    ('+', [0, 0, 0, 0, 1, 0, 1, 1, 1, 0, 1, 0, 0, 0, 0]),
];

fn draw_glyph(px: &mut [u8], size: u32, ox: f32, oy: f32, cell: f32, ch: char, rgb: [u8; 3]) {
    let Some((_, bits)) = GLYPHS.iter().find(|(c, _)| *c == ch) else {
        return;
    };
    for row in 0..5 {
        for col in 0..3 {
            if bits[row * 3 + col] == 0 {
                continue;
            }
            let x0 = ox + col as f32 * cell;
            let y0 = oy + row as f32 * cell;
            fill_rect(px, size, x0, y0, cell, cell, rgb);
        }
    }
}

fn fill_rect(px: &mut [u8], size: u32, x0: f32, y0: f32, w: f32, h: f32, rgb: [u8; 3]) {
    let xmin = x0.floor().max(0.0) as u32;
    let ymin = y0.floor().max(0.0) as u32;
    let xmax = (x0 + w).ceil().min(size as f32) as u32;
    let ymax = (y0 + h).ceil().min(size as f32) as u32;
    for y in ymin..ymax {
        for x in xmin..xmax {
            let cover = rect_cover(x, y, x0, y0, w, h);
            if cover > 0.0 {
                blend(pixel(px, size, x, y), rgb, cover);
            }
        }
    }
}

fn circle_cover(x: u32, y: u32, cx: f32, cy: f32, radius: f32) -> f32 {
    let dx = x as f32 + 0.5 - cx;
    let dy = y as f32 + 0.5 - cy;
    let dist = (dx * dx + dy * dy).sqrt();
    (radius + 0.5 - dist).clamp(0.0, 1.0)
}

fn round_rect_cover(x: u32, y: u32, rx: f32, ry: f32, w: f32, h: f32, radius: f32) -> f32 {
    let px = x as f32 + 0.5;
    let py = y as f32 + 0.5;
    if !(rx..=rx + w).contains(&px) || !(ry..=ry + h).contains(&py) {
        return 0.0;
    }
    let radius = radius.min(w / 2.0).min(h / 2.0);
    let cx = px.clamp(rx + radius, rx + w - radius);
    let cy = py.clamp(ry + radius, ry + h - radius);
    let dx = px - cx;
    let dy = py - cy;
    let dist = (dx * dx + dy * dy).sqrt();
    if dist <= radius {
        return (radius + 0.5 - dist).clamp(0.0, 1.0).max(0.6);
    }
    0.0
}

fn rect_cover(x: u32, y: u32, x0: f32, y0: f32, w: f32, h: f32) -> f32 {
    let l = (x as f32 + 1.0).min(x0 + w) - (x as f32).max(x0);
    let t = (y as f32 + 1.0).min(y0 + h) - (y as f32).max(y0);
    (l.max(0.0) * t.max(0.0)).clamp(0.0, 1.0)
}

fn dist_to_segment(px: f32, py: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        ((px - x1) * dx + (py - y1) * dy) / len2
    }
    .clamp(0.0, 1.0);
    let sx = x1 + t * dx;
    let sy = y1 + t * dy;
    let ex = px - sx;
    let ey = py - sy;
    (ex * ex + ey * ey).sqrt()
}

fn pixel(px: &mut [u8], size: u32, x: u32, y: u32) -> &mut [u8] {
    let i = ((y * size + x) * 4) as usize;
    &mut px[i..i + 4]
}

fn blend(px: &mut [u8], rgb: [u8; 3], cover: f32) {
    let src_a = cover.clamp(0.0, 1.0);
    let dst_a = px[3] as f32 / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a <= 0.0 {
        return;
    }
    for (i, channel) in rgb.iter().enumerate() {
        let src = *channel as f32 / 255.0;
        let dst = px[i] as f32 / 255.0;
        let out = (src * src_a + dst * dst_a * (1.0 - src_a)) / out_a;
        px[i] = (out * 255.0 + 0.5) as u8;
    }
    px[3] = (out_a * 255.0 + 0.5) as u8;
}

/// tray-icon 0.24 drops Click when `Shell_NotifyIconGetRect` fails. Subclass the
/// tray hwnd and treat that as ShowOnly + work-area (no toggle-fight).
#[cfg(windows)]
mod overflow_hook {
    use std::sync::{Arc, Mutex};

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::UI::Shell::{Shell_NotifyIconGetRect, NOTIFYICONIDENTIFIER};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, SetWindowLongPtrW, GWLP_WNDPROC, WM_LBUTTONDOWN, WM_LBUTTONUP,
        WM_RBUTTONDOWN, WM_RBUTTONUP, WNDPROC,
    };

    use super::ClickButton;

    const WM_USER_TRAYICON: u32 = 6002;

    static ORIG: Mutex<isize> = Mutex::new(0);
    static HANDLER: Mutex<Option<Arc<dyn Fn(ClickButton, bool) + Send + Sync>>> = Mutex::new(None);

    pub fn install(hwnd: isize, on_overflow: impl Fn(ClickButton, bool) + Send + Sync + 'static) {
        *HANDLER.lock().expect("overflow handler") = Some(Arc::new(on_overflow));
        unsafe {
            let hwnd = hwnd as HWND;
            let prev = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, hook as usize as isize);
            *ORIG.lock().expect("overflow orig") = prev;
        }
    }

    unsafe extern "system" fn hook(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_USER_TRAYICON {
            let click = match lparam as u32 {
                WM_LBUTTONDOWN => Some((ClickButton::Left, true)),
                WM_LBUTTONUP => Some((ClickButton::Left, false)),
                WM_RBUTTONDOWN => Some((ClickButton::Right, true)),
                WM_RBUTTONUP => Some((ClickButton::Right, false)),
                _ => None,
            };
            if let Some((button, down)) = click {
                if notify_rect_failed(hwnd) {
                    if let Some(handler) = HANDLER.lock().expect("overflow handler").clone() {
                        handler(button, down);
                    }
                }
            }
        }
        let orig = *ORIG.lock().expect("overflow orig");
        if orig == 0 {
            return 0;
        }
        let prev: WNDPROC = std::mem::transmute(orig);
        CallWindowProcW(prev, hwnd, msg, wparam, lparam)
    }

    fn notify_rect_failed(hwnd: HWND) -> bool {
        for uid in 0..32u32 {
            let nid = NOTIFYICONIDENTIFIER {
                cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
                hWnd: hwnd,
                uID: uid,
                guidItem: unsafe { std::mem::zeroed() },
            };
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if unsafe { Shell_NotifyIconGetRect(&nid, &mut rect) } >= 0
                && rect.right > rect.left
                && rect.bottom > rect.top
            {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ExpectedStatus, HttpMethod, Service};
    use chrono::{Duration as ChronoDuration, TimeZone, Utc};

    fn view(id: &str, state: UiState, paused: bool, snoozed: bool) -> ServiceView {
        let now = Utc.with_ymd_and_hms(2026, 8, 18, 12, 0, 0).unwrap();
        ServiceView {
            service: Service {
                id: id.into(),
                name: id.into(),
                url: "https://example.test/health".into(),
                method: HttpMethod::Get,
                headers: vec![],
                body: None,
                interval_sec: 60,
                timeout_ms: 10_000,
                expected_status: ExpectedStatus::TwoXx,
                assertions: vec![],
                max_latency_ms: None,
                action_url: None,
                notify: true,
                always_alert: false,
                paused,
                follow_redirects: true,
                fail_threshold: None,
                group: None,
                created_at: now,
                updated_at: now,
            },
            headers: vec![],
            state,
            snooze_until: snoozed.then(|| now + ChronoDuration::hours(1)),
            keychain_identity_changed: None,
            last_result: None,
            last_check_at: None,
            down_since: None,
            degraded_since: None,
            down_clock_adjust_ms: 0,
            consecutive_hard_fails: 0,
            sparkline24: vec![],
        }
    }

    fn mark(services: &[ServiceView], offline: bool, poller_dead: bool) -> TrayMark {
        mark_from(TraySnapshot {
            services,
            offline,
            poller_dead,
        })
    }

    #[test]
    fn empty_all_paused_all_pending_are_hollow() {
        assert_eq!(mark(&[], false, false), TrayMark::Hollow);
        assert_eq!(
            mark(&[view("a", UiState::Paused, true, false)], false, false),
            TrayMark::Hollow
        );
        assert_eq!(
            mark(
                &[
                    view("a", UiState::Pending, false, false),
                    view("b", UiState::Pending, false, false)
                ],
                false,
                false
            ),
            TrayMark::Hollow
        );
    }

    #[test]
    fn healthy_requires_an_unpaused_non_pending() {
        assert_eq!(
            mark(&[view("a", UiState::Healthy, false, false)], false, false),
            TrayMark::Healthy
        );
        assert_eq!(
            mark(
                &[
                    view("a", UiState::Healthy, false, false),
                    view("b", UiState::Pending, false, false)
                ],
                false,
                false
            ),
            TrayMark::Healthy
        );
    }

    #[test]
    fn worst_of_down_beats_degraded_beats_healthy() {
        let mixed = [
            view("ok", UiState::Healthy, false, false),
            view("slow", UiState::Degraded, false, false),
        ];
        assert_eq!(mark(&mixed, false, false), TrayMark::Degraded);
        let with_down = [
            view("ok", UiState::Healthy, false, false),
            view("slow", UiState::Degraded, false, false),
            view("d1", UiState::Down, false, false),
            view("d2", UiState::Down, false, false),
        ];
        assert_eq!(mark(&with_down, false, false), TrayMark::Down { count: 2 });
    }

    #[test]
    fn paused_and_pending_do_not_count() {
        let services = [
            view("paused-down", UiState::Paused, true, false),
            view("pending", UiState::Pending, false, false),
            view("ok", UiState::Healthy, false, false),
        ];
        assert_eq!(mark(&services, false, false), TrayMark::Healthy);
        let only_paused_down = [view("paused-down", UiState::Down, true, false)];
        assert_eq!(mark(&only_paused_down, false, false), TrayMark::Hollow);
    }

    #[test]
    fn snooze_does_not_change_the_mark() {
        let down = [view("pay", UiState::Down, false, true)];
        assert_eq!(mark(&down, false, false), TrayMark::Down { count: 1 });
        let degraded = [view("api", UiState::Degraded, false, true)];
        assert_eq!(mark(&degraded, false, false), TrayMark::Degraded);
    }

    #[test]
    fn offline_overrides_color_poller_dead_overrides_all() {
        let down = [view("pay", UiState::Down, false, false)];
        assert_eq!(mark(&down, true, false), TrayMark::Offline);
        assert_eq!(mark(&down, true, true), TrayMark::PollerDead);
        assert_eq!(mark(&[], false, true), TrayMark::PollerDead);
    }

    #[test]
    fn click_protocol_suppresses_blur_and_toggles_on_up() {
        let mut proto = ClickProtocol::default();
        let t0 = Instant::now();
        proto.on_down(ClickButton::Left, t0);
        assert!(proto.should_suppress_blur(t0 + Duration::from_millis(100)));
        assert!(!proto.should_suppress_blur(t0 + Duration::from_millis(251)));
        assert_eq!(proto.on_up(ClickButton::Left, false), ClickOutcome::Toggle);
    }

    #[test]
    fn right_click_then_left_does_not_toggle() {
        let mut proto = ClickProtocol::default();
        let t0 = Instant::now();
        proto.on_down(ClickButton::Right, t0);
        proto.on_down(ClickButton::Left, t0);
        assert_eq!(proto.on_up(ClickButton::Left, false), ClickOutcome::None);
        proto.on_down(ClickButton::Left, t0);
        assert_eq!(proto.on_up(ClickButton::Left, false), ClickOutcome::Toggle);
    }

    #[test]
    fn dismissed_menu_after_suppress_toggles() {
        let mut proto = ClickProtocol::default();
        let t0 = Instant::now();
        proto.on_down(ClickButton::Right, t0);
        proto.on_down(ClickButton::Left, t0 + Duration::from_millis(251));
        assert_eq!(proto.on_up(ClickButton::Left, false), ClickOutcome::Toggle);
    }

    #[test]
    fn overflow_show_only_does_not_hide() {
        let mut proto = ClickProtocol::default();
        proto.on_down(ClickButton::Left, Instant::now());
        assert_eq!(proto.on_up(ClickButton::Left, true), ClickOutcome::ShowOnly);
    }

    #[test]
    fn painter_sizes_and_tokens() {
        for &(size, logical) in &[(18, 18.0), (36, 18.0), (16, 16.0), (32, 16.0)] {
            let rgba = paint_mark(TrayMark::Healthy, size, logical);
            assert_eq!(rgba.len(), (size * size * 4) as usize);
            let i = ((size / 2 * size + size / 2) * 4) as usize;
            assert!(rgba[i + 3] > 200, "center filled");
            assert!(rgba[i + 1] > rgba[i], "ok green");
        }

        let hollow = paint_mark(TrayMark::Hollow, 36, 18.0);
        let c = ((18 * 36 + 18) * 4) as usize;
        assert!(hollow[c + 3] < 40, "hollow center empty");

        let dead = paint_mark(TrayMark::PollerDead, 36, 18.0);
        assert!(dead.chunks(4).any(|p| p[0] > 180 && p[3] > 180));

        let down = paint_mark(TrayMark::Down { count: 2 }, 36, 18.0);
        let healthy = paint_mark(TrayMark::Healthy, 36, 18.0);
        assert_ne!(down, healthy);
    }

    #[test]
    fn work_area_fallback_is_bottom_right_minus_inset() {
        assert_eq!(
            work_area_anchor((0, 0, 1920, 1080), 372, 480, 12, false),
            (1920 - 372 - 12, 1080 - 480 - 12)
        );
        let secondary = (1920, 0, 1440, 900);
        assert_eq!(
            work_area_anchor(secondary, 372, 480, 12, false),
            (1920 + 1440 - 372 - 12, 900 - 480 - 12)
        );
    }

    #[test]
    fn macos_first_run_fallback_is_top_right() {
        assert_eq!(
            work_area_anchor((0, 0, 1920, 1080), 372, 480, 12, true),
            (1920 - 372 - 12, 12)
        );
    }

    #[test]
    fn flyout_inside_work_area_is_overflow_taskbar_is_not() {
        let work = (0, 0, 1920, 1040);
        let taskbar = (1880, 1040, 24, 40);
        let flyout = (1700, 800, 24, 24);
        assert!(icon_on_taskbar(taskbar, work));
        assert!(!icon_on_taskbar(flyout, work));
        let empty = tauri::Rect {
            position: Position::Physical(PhysicalPosition::new(0, 0)),
            size: Size::Physical(tauri::PhysicalSize::new(0, 0)),
        };
        assert!(rect_is_empty(&empty));
    }

    #[test]
    fn first_run_suppresses_blur_until_focused() {
        let tray = TrayHandle::new();
        assert!(!tray.should_suppress_blur());
        tray.arm_first_run_show();
        assert!(tray.should_suppress_blur());
        tray.note_popover_focused();
        std::thread::sleep(Duration::from_millis(260));
        assert!(!tray.should_suppress_blur());
    }

    #[test]
    fn clamp_keeps_popover_on_the_same_monitor() {
        let left = (0, 0, 1920, 1080);
        let (x, y) = clamp_to_work_area(3000, 20, 372, 480, left, 12);
        assert_eq!(x, 1920 - 372 - 12);
        assert_eq!(y, 20);
        let right = (1920, 0, 1440, 900);
        let (x, y) = clamp_to_work_area(100, 10, 372, 480, right, 12);
        assert_eq!(x, 1920 + 12);
        assert_eq!(y, 12);
    }

    #[test]
    fn monitor_containing_picks_the_display_under_the_point() {
        let monitors = [(0, 0, 1920, 1080), (1920, 0, 1440, 900)];
        assert_eq!(monitor_containing(&monitors, 10, 10), Some(0));
        assert_eq!(monitor_containing(&monitors, 2000, 100), Some(1));
        assert_eq!(monitor_containing(&monitors, -20, 10), None);
    }

    #[test]
    fn remembers_non_overflow_tray_rect() {
        let tray = TrayHandle::new();
        assert!(tray.last_rect().is_none());
        let overflow = tauri::Rect {
            position: Position::Physical(PhysicalPosition::new(0, 0)),
            size: Size::Physical(tauri::PhysicalSize::new(0, 0)),
        };
        tray.remember_rect(&overflow);
        assert!(tray.last_rect().is_none());
        let rect = tauri::Rect {
            position: Position::Physical(PhysicalPosition::new(10, 20)),
            size: Size::Physical(tauri::PhysicalSize::new(18, 18)),
        };
        tray.remember_rect(&rect);
        match tray.last_rect().expect("stored").position {
            Position::Physical(pos) => {
                assert_eq!(pos.x, 10);
                assert_eq!(pos.y, 20);
            }
            other => panic!("expected physical, got {other:?}"),
        }
    }

    #[test]
    fn hook_sets_poller_dead_mark() {
        let tray = TrayHandle::new();
        tray.apply_services(&[view("ok", UiState::Healthy, false, false)]);
        assert_eq!(tray.mark(), TrayMark::Healthy);
        let hook = tray.poller_dead_hook();
        hook(true);
        assert_eq!(tray.mark(), TrayMark::PollerDead);
        hook(false);
        assert_eq!(tray.mark(), TrayMark::Healthy);
    }

    #[test]
    fn repaint_drops_lock_before_apply_icon() {
        let tray = TrayHandle::new();
        let painted = tray.clone();
        tray.bind_icon(Arc::new(move |_| {
            let _ = painted.mark();
        }));
        tray.set_poller_dead(true);
        assert_eq!(tray.mark(), TrayMark::PollerDead);
        tray.set_poller_dead(false);
        tray.apply_services(&[view("ok", UiState::Healthy, false, false)]);
        assert_eq!(tray.mark(), TrayMark::Healthy);
        tray.set_offline(true);
        assert_eq!(tray.mark(), TrayMark::Offline);
    }
}
