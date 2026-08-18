use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{oneshot, Notify, Semaphore};
use tokio::task::{AbortHandle, JoinHandle};
use tokio::time::sleep;

use crate::domain::view::{assemble_view, compact_sample};
use crate::domain::MachineStatus;
use crate::domain::{
    AppSettings, CheckEvidence, CheckResult, ErrorKind, OutcomeClass, RuntimeState, Service,
    ServiceStatus, ServiceView,
};
use crate::eval::{evaluate_at, outcome_of};
use crate::notify::{
    down_body, down_title, flush_quiet_queue, in_quiet_hours, DownGrouper, Emit, Notification,
    Notifier, NotifyPolicy, QueueOp, QueuedDown, QuietQueue,
};
use crate::poller::offline::{
    host_of, in_wake_grace, is_overdue, is_transport_error, offline_adjust_ms, OfflineDetector,
    OfflineTransition, RESUME_SETTLE,
};
use crate::poller::state_machine::{fail_threshold, on_result, ProbeEvent};
use crate::poller::HttpClient;
use crate::store::{History, SecretStore, StoreError};

pub const CONCURRENCY: usize = 4;
pub const STAGGER_CAP: Duration = Duration::from_secs(15);
pub const CHECK_ALL_GAP: Duration = Duration::from_millis(50);
pub const VIEW_COALESCE: Duration = Duration::from_millis(100);
pub const GROUPER_TICK: Duration = Duration::from_millis(200);
pub const JITTER_FRAC: f64 = 0.10;
pub const WATCHDOG_RESET: Duration = Duration::from_secs(60);

pub trait PulseEvents: Send + Sync {
    fn emit_services(&self, views: &[ServiceView]);
    fn emit_poller_dead(&self, at: DateTime<Utc>);
    fn emit_offline(&self, _offline: bool) {}
}

pub struct NoopEvents;

impl PulseEvents for NoopEvents {
    fn emit_services(&self, _views: &[ServiceView]) {}
    fn emit_poller_dead(&self, _at: DateTime<Utc>) {}
}

pub struct ChannelEvents {
    pub services: tokio::sync::mpsc::UnboundedSender<Vec<ServiceView>>,
    pub dead: tokio::sync::mpsc::UnboundedSender<DateTime<Utc>>,
}

impl PulseEvents for ChannelEvents {
    fn emit_services(&self, views: &[ServiceView]) {
        let _ = self.services.send(views.to_vec());
    }
    fn emit_poller_dead(&self, at: DateTime<Utc>) {
        let _ = self.dead.send(at);
    }
}

pub struct TauriEvents<R: tauri::Runtime>(pub tauri::AppHandle<R>);

impl<R: tauri::Runtime> PulseEvents for TauriEvents<R> {
    fn emit_services(&self, views: &[ServiceView]) {
        use tauri::Emitter;
        let _ = self.0.emit("pulse://services", views);
    }

    fn emit_poller_dead(&self, at: DateTime<Utc>) {
        use tauri::Emitter;
        #[derive(Clone, serde::Serialize)]
        struct Dead {
            at: DateTime<Utc>,
        }
        let _ = self.0.emit("pulse://poller-dead", Dead { at });
    }

    fn emit_offline(&self, offline: bool) {
        use tauri::Emitter;
        #[derive(Clone, serde::Serialize)]
        struct Offline {
            offline: bool,
        }
        let _ = self.0.emit("pulse://offline", Offline { offline });
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("service not found")]
    NotFound,
    #[error("check canceled")]
    Canceled,
    #[error("invalid snooze timestamp")]
    InvalidSnooze,
    #[error("could not open URL")]
    Open,
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl serde::Serialize for SchedulerError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// `i * min(interval) / n`, capped at 15s. First service (i=0) fires immediately.
pub fn start_stagger(index: usize, n: usize, min_interval: Duration) -> Duration {
    if n <= 1 || index == 0 {
        return Duration::ZERO;
    }
    let secs = min_interval.as_secs_f64() * (index as f64) / (n as f64);
    Duration::from_secs_f64(secs.min(STAGGER_CAP.as_secs_f64()))
}

/// ±10% of `interval`. Deterministic in `seed` so tests are not flaky.
pub fn with_jitter(interval: Duration, seed: u64) -> Duration {
    let unit = splitmix64(seed) as f64 / (u64::MAX as f64);
    let factor = 1.0 + (unit * 2.0 - 1.0) * JITTER_FRAC;
    Duration::from_secs_f64(interval.as_secs_f64() * factor)
}

fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// One restart. A second death inside 60s stays in `poller_dead`; after 60s uptime allow another.
pub fn should_restart(death_count: u32, uptime: Duration) -> bool {
    death_count == 1 || uptime >= WATCHDOG_RESET
}

pub struct SchedulerConfig {
    pub services: Vec<Service>,
    pub settings: AppSettings,
    pub history: History,
    pub secrets: Arc<SecretStore>,
    pub events: Arc<dyn PulseEvents>,
    pub notifier: Box<dyn Notifier + Send>,
    pub enable_jitter: bool,
    pub on_poller_dead: Arc<dyn Fn(bool) + Send + Sync>,
}

struct Slot {
    service: Service,
    check_now: Arc<Notify>,
    abort: Option<AbortHandle>,
    waiters: Vec<oneshot::Sender<Result<CheckResult, SchedulerError>>>,
    checking: bool,
}

struct Inner {
    slots: Mutex<HashMap<String, Slot>>,
    semaphore: Arc<Semaphore>,
    history: Mutex<History>,
    secrets: Arc<SecretStore>,
    settings: RwLock<AppSettings>,
    http: HttpClient,
    events: Arc<dyn PulseEvents>,
    notifier: Mutex<Box<dyn Notifier + Send>>,
    grouper: Mutex<DownGrouper>,
    quiet: Mutex<QuietQueue>,
    dirty: Notify,
    stop: Notify,
    stopped: AtomicBool,
    poller_dead: AtomicBool,
    enable_jitter: bool,
    on_poller_dead: Arc<dyn Fn(bool) + Send + Sync>,
    checks: AtomicU64,
    helpers: Mutex<Vec<AbortHandle>>,
    child_panic: Notify,
    child_panic_gen: AtomicU64,
    offline: Mutex<OfflineDetector>,
    wake_at_ms: AtomicI64,
    wake_gen: AtomicU64,
}

pub struct Scheduler {
    inner: Arc<Inner>,
}

#[derive(Clone)]
pub struct SchedulerHandle {
    inner: Arc<Inner>,
}

impl Scheduler {
    pub fn new(config: SchedulerConfig) -> Result<Self, StoreError> {
        let mut slots = HashMap::new();
        for service in config.services {
            slots.insert(
                service.id.clone(),
                Slot {
                    service,
                    check_now: Arc::new(Notify::new()),
                    abort: None,
                    waiters: Vec::new(),
                    checking: false,
                },
            );
        }
        Ok(Self {
            inner: Arc::new(Inner {
                slots: Mutex::new(slots),
                semaphore: Arc::new(Semaphore::new(CONCURRENCY)),
                history: Mutex::new(config.history),
                secrets: config.secrets,
                settings: RwLock::new(config.settings),
                http: HttpClient::new(),
                events: config.events,
                notifier: Mutex::new(config.notifier),
                grouper: Mutex::new(DownGrouper::new()),
                quiet: Mutex::new(QuietQueue::new()),
                dirty: Notify::new(),
                stop: Notify::new(),
                stopped: AtomicBool::new(false),
                poller_dead: AtomicBool::new(false),
                enable_jitter: config.enable_jitter,
                on_poller_dead: config.on_poller_dead,
                checks: AtomicU64::new(0),
                helpers: Mutex::new(Vec::new()),
                child_panic: Notify::new(),
                child_panic_gen: AtomicU64::new(0),
                offline: Mutex::new(OfflineDetector::new()),
                wake_at_ms: AtomicI64::new(0),
                wake_gen: AtomicU64::new(0),
            }),
        })
    }

    pub fn handle(&self) -> SchedulerHandle {
        SchedulerHandle {
            inner: Arc::clone(&self.inner),
        }
    }

    pub fn poller_dead(&self) -> bool {
        self.inner.poller_dead.load(Ordering::SeqCst)
    }

    /// Supervisor + watchdog. Call from a Tokio runtime.
    pub async fn run(self) {
        let inner = Arc::clone(&self.inner);
        let hook = Arc::clone(&inner.on_poller_dead);
        let events = Arc::clone(&inner.events);
        supervise(
            {
                let inner = Arc::clone(&inner);
                move || {
                    let inner = Arc::clone(&inner);
                    async move { inner.supervise().await }
                }
            },
            move |at, restarting| {
                tracing::error!(event = "poller_dead", restarting, "poller task ended");
                inner.poller_dead.store(true, Ordering::SeqCst);
                events.emit_poller_dead(at);
                hook(true);
            },
        )
        .await;
    }
}

impl SchedulerHandle {
    pub fn views(&self) -> Vec<ServiceView> {
        self.inner.views()
    }

    pub fn view(&self, id: &str) -> Result<ServiceView, SchedulerError> {
        self.inner.view(id).ok_or(SchedulerError::NotFound)
    }

    pub fn poller_dead(&self) -> bool {
        self.inner.poller_dead.load(Ordering::SeqCst)
    }

    pub fn update_settings(&self, settings: AppSettings) {
        *self.inner.settings.write().expect("settings lock") = settings;
        self.inner.maybe_flush_quiet(Utc::now());
    }

    pub fn upsert(&self, service: Service) {
        self.inner.upsert(service);
        self.inner.mark_dirty();
    }

    pub fn remove(&self, id: &str) {
        self.inner.abort_one(id);
        self.inner.slots.lock().expect("slots lock").remove(id);
        self.inner.quiet.lock().expect("quiet lock").recover(id);
        self.inner.mark_dirty();
    }

    pub fn clear_services(&self) {
        self.inner.abort_all();
        self.inner.slots.lock().expect("slots lock").clear();
        self.inner.mark_dirty();
    }

    pub fn set_paused(&self, id: &str, paused: bool) -> Result<ServiceView, SchedulerError> {
        self.inner.set_paused(id, paused)?;
        self.inner.mark_dirty();
        self.view(id)
    }

    pub fn set_snooze(
        &self,
        id: &str,
        until: Option<DateTime<Utc>>,
    ) -> Result<ServiceView, SchedulerError> {
        self.inner.set_snooze(id, until)?;
        self.inner.mark_dirty();
        self.view(id)
    }

    pub async fn check_now(&self, id: &str) -> Result<CheckResult, SchedulerError> {
        enum Action {
            Direct,
            Wait {
                wake: bool,
                rx: oneshot::Receiver<Result<CheckResult, SchedulerError>>,
            },
        }
        let action = {
            let mut slots = self.inner.slots.lock().expect("slots lock");
            let slot = slots.get_mut(id).ok_or(SchedulerError::NotFound)?;
            if slot.service.paused || slot.abort.is_none() {
                Action::Direct
            } else {
                let (tx, rx) = oneshot::channel();
                slot.waiters.push(tx);
                Action::Wait {
                    wake: !slot.checking,
                    rx,
                }
            }
        };
        match action {
            Action::Direct => {
                let service = self
                    .inner
                    .clone_service(id)
                    .ok_or(SchedulerError::NotFound)?;
                self.inner.run_check(&service).await
            }
            Action::Wait { wake, rx } => {
                if wake {
                    self.inner.wake(id);
                }
                rx.await.map_err(|_| SchedulerError::Canceled)?
            }
        }
    }

    pub async fn check_all(&self) {
        let ids = self.inner.unpaused_ids();
        for (i, id) in ids.into_iter().enumerate() {
            if i > 0 {
                sleep(CHECK_ALL_GAP).await;
            }
            if self.inner.stopped.load(Ordering::SeqCst) {
                return;
            }
            let wake = {
                let slots = self.inner.slots.lock().expect("slots lock");
                slots.get(&id).is_some_and(|slot| {
                    slot.abort.is_some() && !slot.service.paused && !slot.checking
                })
            };
            if wake {
                self.inner.wake(&id);
            }
        }
    }

    pub fn with_history<T>(&self, f: impl FnOnce(&History) -> T) -> T {
        let history = self.inner.history.lock().expect("history lock");
        f(&history)
    }

    pub fn shutdown(&self) {
        self.inner.shutdown();
    }

    pub fn is_offline(&self) -> bool {
        self.inner
            .offline
            .lock()
            .expect("offline lock")
            .is_offline()
    }

    pub fn on_os_sleep(&self) {
        self.inner.on_sleep(Utc::now());
    }

    pub fn on_os_wake(&self) {
        let inner = Arc::clone(&self.inner);
        tauri::async_runtime::spawn(async move {
            inner.on_wake().await;
        });
    }

    pub async fn resume_from_wake(&self) {
        self.inner.on_wake().await;
    }
}

impl Inner {
    fn wake(&self, id: &str) {
        if let Some(slot) = self.slots.lock().expect("slots lock").get(id) {
            slot.check_now.notify_one();
        }
    }

    fn all_ids(&self) -> Vec<String> {
        self.slots
            .lock()
            .expect("slots lock")
            .keys()
            .cloned()
            .collect()
    }

    fn note_wake(&self, now: DateTime<Utc>) {
        self.wake_at_ms
            .store(now.timestamp_millis(), Ordering::SeqCst);
    }

    fn in_wake_grace(&self, now: DateTime<Utc>) -> bool {
        let ms = self.wake_at_ms.load(Ordering::SeqCst);
        if ms <= 0 {
            return false;
        }
        DateTime::from_timestamp_millis(ms).is_some_and(|wake| in_wake_grace(now, wake))
    }

    fn on_sleep(&self, now: DateTime<Utc>) {
        let ids = self.all_ids();
        let history = self.history.lock().expect("history lock");
        for id in ids {
            let _ = history.apply_sleep(&id, now);
        }
    }

    fn apply_wakes(&self, now: DateTime<Utc>) {
        let ids = self.all_ids();
        let history = self.history.lock().expect("history lock");
        for id in ids {
            let _ = history.apply_wake(&id, now);
        }
    }

    fn maybe_flush_quiet(&self, now: DateTime<Utc>) {
        let settings = self.settings.read().expect("settings lock").clone();
        let in_quiet = settings
            .quiet_hours
            .as_ref()
            .is_some_and(|hours| in_quiet_hours(hours, now));
        if in_quiet {
            return;
        }
        if !settings.notifications {
            self.quiet.lock().expect("quiet lock").retain(|_| false);
            return;
        }
        // History then quiet — same order as persist+dequeue in run_check.
        let events = {
            let history = self.history.lock().expect("history lock");
            let mut queue = self.quiet.lock().expect("quiet lock");
            queue.retain(|entry| match history.load_runtime(&entry.service_id) {
                Ok(runtime) => runtime.status == MachineStatus::Down && !runtime.is_snoozed(now),
                Err(_) => false,
            });
            flush_quiet_queue(&mut queue)
        };
        if events.is_empty() {
            return;
        }
        let mut notifier = self.notifier.lock().expect("notifier lock");
        for item in events {
            notifier.notify(item);
        }
    }

    fn stamp_offline_enter(&self) {
        let ids = self.all_ids();
        let snaps = {
            let history = self.history.lock().expect("history lock");
            let mut snaps = HashMap::new();
            for id in ids {
                if let Ok(runtime) = history.load_runtime(&id) {
                    if runtime.status == MachineStatus::Down {
                        snaps.insert(id, runtime.down_clock_adjust_ms);
                    }
                }
            }
            snaps
        };
        self.offline
            .lock()
            .expect("offline lock")
            .stamp_enter_adjusts(snaps);
    }

    fn apply_offline_clock(
        history: &History,
        snaps: &HashMap<String, u64>,
        ids: &[String],
        entered_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) {
        for id in ids {
            let Ok(mut runtime) = history.load_runtime(id) else {
                continue;
            };
            if runtime.status != MachineStatus::Down {
                continue;
            }
            let at_enter = snaps
                .get(id)
                .copied()
                .unwrap_or(runtime.down_clock_adjust_ms);
            let add = offline_adjust_ms(
                entered_at,
                now,
                runtime.paused_at,
                runtime.slept_at,
                runtime.down_clock_adjust_ms,
                at_enter,
            );
            if add > 0 {
                runtime.down_clock_adjust_ms = runtime.down_clock_adjust_ms.saturating_add(add);
                let _ = history.put_runtime(id, &runtime);
            }
        }
    }

    async fn on_wake(self: &Arc<Self>) {
        let gen = self.wake_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let now = Utc::now();
        self.note_wake(now);
        self.apply_wakes(now);
        // In-flight HTTP dies with the task; canceled is a state-machine no-op.
        self.abort_all();
        tokio::select! {
            _ = sleep(RESUME_SETTLE) => {}
            _ = self.cancelled() => return,
        }
        if self.wake_gen.load(Ordering::SeqCst) != gen {
            return;
        }
        self.maybe_flush_quiet(Utc::now());
        if !self.stopped.load(Ordering::SeqCst) {
            self.spawn_all();
        }
    }

    fn mark_dirty(&self) {
        self.dirty.notify_one();
    }

    fn shutdown(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        // notify_one stores a permit so a not-yet-waiting cancelled() cannot miss the stop.
        self.stop.notify_waiters();
        self.stop.notify_one();
        self.dirty.notify_waiters();
        self.dirty.notify_one();
        self.abort_all();
    }

    async fn cancelled(&self) {
        loop {
            if self.stopped.load(Ordering::SeqCst) {
                return;
            }
            self.stop.notified().await;
        }
    }

    fn clone_service(&self, id: &str) -> Option<Service> {
        self.slots
            .lock()
            .expect("slots lock")
            .get(id)
            .map(|slot| slot.service.clone())
    }

    fn unpaused_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .slots
            .lock()
            .expect("slots lock")
            .values()
            .filter(|slot| !slot.service.paused)
            .map(|slot| slot.service.id.clone())
            .collect();
        ids.sort();
        ids
    }

    fn views(&self) -> Vec<ServiceView> {
        let slots = self.slots.lock().expect("slots lock");
        let history = self.history.lock().expect("history lock");
        let now = Utc::now();
        let mut views = Vec::with_capacity(slots.len());
        for slot in slots.values() {
            let runtime = history
                .load_runtime(&slot.service.id)
                .unwrap_or_else(|_| RuntimeState::pending());
            let last = history.last_result(&slot.service.id).ok().flatten();
            let samples = history
                .samples_24h(&slot.service.id, now)
                .unwrap_or_default();
            let identity = self.secrets.service_identity_changed(&slot.service.id);
            views.push(assemble_view(
                &slot.service,
                &runtime,
                last.as_ref(),
                &samples,
                identity,
                &self.secrets,
            ));
        }
        views.sort_by(|a, b| a.service.id.cmp(&b.service.id));
        views
    }

    fn view(&self, id: &str) -> Option<ServiceView> {
        self.views().into_iter().find(|view| view.service.id == id)
    }

    fn min_unpaused_interval(&self) -> Duration {
        let min = self
            .slots
            .lock()
            .expect("slots lock")
            .values()
            .filter(|slot| !slot.service.paused)
            .map(|slot| slot.service.interval_sec)
            .min()
            .unwrap_or(60);
        Duration::from_secs(u64::from(min))
    }

    fn stagger_for(&self, id: &str) -> Duration {
        let ids = self.unpaused_ids();
        let n = ids.len();
        let index = ids
            .iter()
            .position(|candidate| candidate == id)
            .unwrap_or(0);
        start_stagger(index, n, self.min_unpaused_interval())
    }

    fn abort_one(&self, id: &str) {
        if let Some(slot) = self.slots.lock().expect("slots lock").get_mut(id) {
            if let Some(abort) = slot.abort.take() {
                abort.abort();
            }
            for waiter in std::mem::take(&mut slot.waiters) {
                let _ = waiter.send(Err(SchedulerError::Canceled));
            }
            slot.checking = false;
        }
    }

    fn abort_all(&self) {
        for slot in self.slots.lock().expect("slots lock").values_mut() {
            if let Some(abort) = slot.abort.take() {
                abort.abort();
            }
            for waiter in std::mem::take(&mut slot.waiters) {
                let _ = waiter.send(Err(SchedulerError::Canceled));
            }
            slot.checking = false;
        }
    }

    fn abort_helpers(&self) {
        for handle in self.helpers.lock().expect("helpers lock").drain(..) {
            handle.abort();
        }
    }

    fn track_loop<F>(self: &Arc<Self>, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(fut);
        self.helpers
            .lock()
            .expect("helpers lock")
            .push(handle.abort_handle());
        self.watch_join(handle);
    }

    fn watch_join(self: &Arc<Self>, handle: JoinHandle<()>) {
        let inner = Arc::clone(self);
        tokio::spawn(async move {
            if matches!(handle.await, Err(error) if error.is_panic()) {
                inner.child_panic_gen.fetch_add(1, Ordering::SeqCst);
                inner.child_panic.notify_waiters();
                inner.child_panic.notify_one();
            }
        });
    }

    fn set_checking(&self, id: &str, checking: bool) {
        if let Some(slot) = self.slots.lock().expect("slots lock").get_mut(id) {
            slot.checking = checking;
        }
    }

    fn finish_check(&self, id: &str, result: Result<CheckResult, SchedulerError>) {
        let waiters = {
            let mut slots = self.slots.lock().expect("slots lock");
            match slots.get_mut(id) {
                Some(slot) => {
                    slot.checking = false;
                    std::mem::take(&mut slot.waiters)
                }
                None => Vec::new(),
            }
        };
        for waiter in waiters {
            let payload = match &result {
                Ok(check) => Ok(check.clone()),
                Err(_) => Err(SchedulerError::Canceled),
            };
            let _ = waiter.send(payload);
        }
    }

    fn upsert(self: &Arc<Self>, service: Service) {
        let id = service.id.clone();
        let paused = service.paused;
        {
            let mut slots = self.slots.lock().expect("slots lock");
            if let Some(slot) = slots.get_mut(&id) {
                if let Some(abort) = slot.abort.take() {
                    abort.abort();
                }
                slot.service = service;
                slot.check_now = Arc::new(Notify::new());
                for waiter in std::mem::take(&mut slot.waiters) {
                    let _ = waiter.send(Err(SchedulerError::Canceled));
                }
                slot.checking = false;
            } else {
                slots.insert(
                    id.clone(),
                    Slot {
                        service,
                        check_now: Arc::new(Notify::new()),
                        abort: None,
                        waiters: Vec::new(),
                        checking: false,
                    },
                );
            }
        }
        if !paused && !self.stopped.load(Ordering::SeqCst) {
            // Save / edit: first poll is async and starts immediately (no start stagger).
            self.spawn_service(id, Duration::ZERO);
        }
    }

    fn set_paused(self: &Arc<Self>, id: &str, paused: bool) -> Result<(), SchedulerError> {
        let now = Utc::now();
        {
            let mut slots = self.slots.lock().expect("slots lock");
            let slot = slots.get_mut(id).ok_or(SchedulerError::NotFound)?;
            slot.service.paused = paused;
        }
        if paused {
            // abort_one drains check_now waiters; a raw abort leaves them parked.
            self.abort_one(id);
        }
        {
            let history = self.history.lock().expect("history lock");
            if paused {
                history.apply_pause(id, now)?;
            } else {
                history.apply_unpause(id, now)?;
            }
        }
        if !paused && !self.stopped.load(Ordering::SeqCst) {
            let delay = self.stagger_for(id);
            self.spawn_service(id.to_string(), delay);
        }
        Ok(())
    }

    fn set_snooze(&self, id: &str, until: Option<DateTime<Utc>>) -> Result<(), SchedulerError> {
        if self.clone_service(id).is_none() {
            return Err(SchedulerError::NotFound);
        }
        self.history
            .lock()
            .expect("history lock")
            .set_snooze(id, until)?;
        if until.is_some() {
            self.quiet.lock().expect("quiet lock").recover(id);
        }
        Ok(())
    }

    async fn supervise(self: Arc<Self>) {
        self.abort_helpers();
        self.abort_all();
        self.stopped.store(false, Ordering::SeqCst);
        self.poller_dead.store(false, Ordering::SeqCst);
        (self.on_poller_dead)(false);
        let panic_gen = self.child_panic_gen.load(Ordering::SeqCst);
        // Kill-during-sleep / missed DidWake: fold leftover slept_at at boot.
        self.apply_wakes(Utc::now());
        self.spawn_all();
        self.track_loop({
            let inner = Arc::clone(&self);
            async move { inner.coalesce_loop().await }
        });
        self.track_loop({
            let inner = Arc::clone(&self);
            async move { inner.prune_loop().await }
        });
        self.track_loop({
            let inner = Arc::clone(&self);
            async move { inner.grouper_loop().await }
        });
        self.mark_dirty();
        tokio::select! {
            _ = self.cancelled() => {}
            _ = self.child_panic.notified() => {
                if self.child_panic_gen.load(Ordering::SeqCst) > panic_gen
                    && !self.stopped.load(Ordering::SeqCst)
                {
                    self.abort_helpers();
                    self.abort_all();
                    panic!("poller child panicked");
                }
            }
        }
        self.abort_helpers();
        self.abort_all();
    }

    fn spawn_all(self: &Arc<Self>) {
        let ids = self.unpaused_ids();
        let n = ids.len();
        let min_interval = self.min_unpaused_interval();
        for (i, id) in ids.into_iter().enumerate() {
            self.spawn_service(id, start_stagger(i, n, min_interval));
        }
    }

    fn spawn_service(self: &Arc<Self>, id: String, delay: Duration) {
        self.abort_one(&id);
        let check_now = {
            let slots = self.slots.lock().expect("slots lock");
            slots.get(&id).map(|slot| Arc::clone(&slot.check_now))
        };
        let Some(check_now) = check_now else {
            return;
        };
        let inner = Arc::clone(self);
        let task_id = id.clone();
        let handle = tokio::spawn(async move {
            inner.service_loop(task_id, delay, check_now).await;
        });
        if let Some(slot) = self.slots.lock().expect("slots lock").get_mut(&id) {
            slot.abort = Some(handle.abort_handle());
            self.watch_join(handle);
        } else {
            handle.abort();
        }
    }

    async fn service_loop(self: Arc<Self>, id: String, delay: Duration, check_now: Arc<Notify>) {
        if !delay.is_zero() {
            tokio::select! {
                _ = sleep(delay) => {}
                _ = check_now.notified() => {}
                _ = self.cancelled() => return,
            }
        }
        loop {
            if self.stopped.load(Ordering::SeqCst) {
                return;
            }
            let Some(service) = self.clone_service(&id) else {
                return;
            };
            if service.paused {
                return;
            }
            self.set_checking(&id, true);
            let result = self.run_check(&service).await;
            self.finish_check(&id, result);
            if self.stopped.load(Ordering::SeqCst) {
                return;
            }
            let interval = Duration::from_secs(u64::from(service.interval_sec));
            // Jitter is on the sleep after a check, never on the start stagger.
            let wait = if self.enable_jitter {
                let seed = self.checks.load(Ordering::Relaxed) ^ fnv(&id);
                with_jitter(interval, seed)
            } else {
                interval
            };
            let next_due = Utc::now() + chrono::Duration::seconds(i64::from(service.interval_sec));
            let elapsed = tokio::select! {
                _ = sleep(wait) => true,
                _ = check_now.notified() => false,
                _ = self.cancelled() => return,
            };
            if is_overdue(Utc::now(), next_due, interval) {
                let now = Utc::now();
                self.note_wake(now);
                self.apply_wakes(now);
                if elapsed {
                    tokio::select! {
                        _ = sleep(RESUME_SETTLE) => {}
                        _ = check_now.notified() => {}
                        _ = self.cancelled() => return,
                    }
                }
            }
        }
    }

    async fn run_check(&self, service: &Service) -> Result<CheckResult, SchedulerError> {
        // Same fair semaphore for live polls and check-now — no priority.
        let permit = tokio::select! {
            permit = self.semaphore.acquire() => {
                permit.map_err(|_| SchedulerError::Canceled)?
            }
            _ = self.cancelled() => return Err(SchedulerError::Canceled),
        };
        let now = Utc::now();
        let (evidence, identity) = match self.secrets.resolve_service(service) {
            Ok(headers) => {
                let identity = self.secrets.service_identity_changed(&service.id);
                let mut map = HashMap::new();
                for header in headers.iter() {
                    if header.secret {
                        map.insert(header.key.clone(), header.value.clone());
                    }
                }
                let raw = self.http.check(service, &map).await;
                (evaluate_at(service, raw, now), identity)
            }
            Err(missing) => (
                CheckEvidence::missing_secret(&missing.key, now),
                missing.identity_changed || self.secrets.service_identity_changed(&service.id),
            ),
        };
        drop(permit);

        let settings = self.settings.read().expect("settings lock").clone();
        let threshold = fail_threshold(service.fail_threshold, settings.fail_threshold);
        let paused = self
            .clone_service(&service.id)
            .map(|current| current.paused)
            .unwrap_or(service.paused);
        let reached = matches!(evidence.outcome, OutcomeClass::Ok | OutcomeClass::Soft);
        let grace_transport = self.in_wake_grace(now)
            && !reached
            && evidence.error_kind.is_some_and(is_transport_error);
        let unpaused = self.unpaused_ids().len();
        // Grace-window NIC errors are not honest; do not feed the offline detector.
        let offline_change = if grace_transport {
            OfflineTransition::None
        } else {
            let mut detector = self.offline.lock().expect("offline lock");
            detector.observe(
                host_of(&service.url).as_deref(),
                evidence.error_kind,
                reached,
                unpaused,
                now,
            )
        };
        match offline_change {
            OfflineTransition::Entered => {
                self.stamp_offline_enter();
                tracing::info!(event = "offline", offline = true, "offline");
                self.events.emit_offline(true);
            }
            OfflineTransition::Exited { .. } => {
                tracing::info!(event = "offline", offline = false, "offline");
                self.events.emit_offline(false);
            }
            OfflineTransition::None => {}
        }
        let offline = self.offline.lock().expect("offline lock").is_offline();
        let event = if offline {
            ProbeEvent::Offline
        } else if grace_transport {
            ProbeEvent::Canceled
        } else {
            ProbeEvent::Applied(outcome_of(&evidence))
        };

        // Snapshot slots / offline before history. Every other path is
        // slots-then-history (views, sleep, wake, stamp_offline_enter).
        let ids = self.all_ids();
        let snaps = if matches!(offline_change, OfflineTransition::Exited { .. }) {
            self.offline
                .lock()
                .expect("offline lock")
                .take_enter_adjusts()
        } else {
            HashMap::new()
        };
        // Fold + load + on_result + persist under one lock so peers cannot
        // overwrite the offline adjust with a stale pre-fold snapshot.
        let (transition, result) = {
            let history = self.history.lock().expect("history lock");
            if let OfflineTransition::Exited { entered_at } = offline_change {
                Self::apply_offline_clock(&history, &snaps, &ids, entered_at, now);
            }
            let mut runtime = history
                .load_runtime(&service.id)
                .unwrap_or_else(|_| RuntimeState::pending());
            let policy = NotifyPolicy {
                notifications: settings.notifications,
                service_notify: service.notify,
                always_alert: service.always_alert,
                in_quiet_hours: settings
                    .quiet_hours
                    .as_ref()
                    .is_some_and(|hours| in_quiet_hours(hours, now)),
                snoozed: runtime.is_snoozed(now),
                keychain_identity_changed: identity,
            };
            let transition = on_result(
                &mut runtime,
                event,
                now,
                threshold,
                paused,
                offline,
                &policy,
            );
            let state = runtime
                .status
                .as_service_status()
                .unwrap_or(ServiceStatus::Healthy);
            let result = CheckResult {
                evidence: evidence.clone(),
                state,
            };
            if transition.applied && !offline {
                let sample = compact_sample(&result);
                history.put_runtime(&service.id, &runtime)?;
                history.put_last_result(&service.id, &result)?;
                history.insert_sample(&service.id, &sample)?;
            }
            // Dequeue under the same history lock as persist so flush cannot
            // snapshot Down after Recovered has already written Healthy.
            if transition.queue != QueueOp::None {
                let (title, body) = if matches!(transition.queue, QueueOp::Enqueue) {
                    (
                        down_title(&service.name),
                        down_body(&evidence, service.timeout_ms),
                    )
                } else {
                    (service.name.clone(), String::new())
                };
                self.quiet.lock().expect("quiet lock").apply(
                    transition.queue,
                    QueuedDown {
                        service_id: service.id.clone(),
                        name: service.name.clone(),
                        title,
                        body,
                    },
                );
            }
            (transition, result)
        };

        if let Some(emit) = transition.emit {
            self.emit_notification(service, &evidence, emit, now);
        }

        self.checks.fetch_add(1, Ordering::Relaxed);
        if offline {
            log_offline(service, service.interval_sec);
        } else {
            log_check(service, &evidence, service.interval_sec);
        }
        self.mark_dirty();
        Ok(result)
    }

    fn emit_notification(
        &self,
        service: &Service,
        evidence: &CheckEvidence,
        emit: Emit,
        now: DateTime<Utc>,
    ) {
        let notification = match emit {
            Emit::Down => Notification::down(
                service.id.clone(),
                &service.name,
                evidence,
                service.timeout_ms,
            ),
            Emit::Recovered { duration_ms } => {
                Notification::recovered(service.id.clone(), &service.name, duration_ms)
            }
        };
        let ready = self
            .grouper
            .lock()
            .expect("grouper lock")
            .push(notification, now);
        let mut notifier = self.notifier.lock().expect("notifier lock");
        for item in ready {
            notifier.notify(item);
        }
    }

    async fn coalesce_loop(self: Arc<Self>) {
        loop {
            tokio::select! {
                _ = self.dirty.notified() => {}
                _ = self.cancelled() => return,
            }
            tokio::select! {
                _ = sleep(VIEW_COALESCE) => {}
                _ = self.cancelled() => return,
            }
            let views = self.views();
            self.events.emit_services(&views);
        }
    }

    async fn grouper_loop(self: Arc<Self>) {
        loop {
            tokio::select! {
                _ = sleep(GROUPER_TICK) => {}
                _ = self.cancelled() => return,
            }
            self.maybe_flush_quiet(Utc::now());
            let ready = self.grouper.lock().expect("grouper lock").poll(Utc::now());
            if ready.is_empty() {
                continue;
            }
            let mut notifier = self.notifier.lock().expect("notifier lock");
            for item in ready {
                notifier.notify(item);
            }
        }
    }

    async fn prune_loop(self: Arc<Self>) {
        loop {
            tokio::select! {
                _ = sleep(Duration::from_secs(600)) => {}
                _ = self.cancelled() => return,
            }
            let _ = self.history.lock().expect("history lock").prune();
        }
    }
}

fn error_kind_name(kind: ErrorKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{kind:?}"))
}

fn fnv(id: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn log_offline(service: &Service, next_sec: u32) {
    tracing::info!(
        id = %service.id,
        name = %service.name,
        outcome = "offline",
        kind = "offline",
        http = 0,
        latency_ms = 0,
        next = next_sec,
        "check"
    );
}

fn log_check(service: &Service, evidence: &CheckEvidence, next_sec: u32) {
    let outcome = match evidence.outcome {
        OutcomeClass::Ok => "ok",
        OutcomeClass::Soft => "soft_fail",
        OutcomeClass::Hard => "hard_fail",
    };
    let kind = evidence.error_kind.map(error_kind_name).unwrap_or_default();
    tracing::info!(
        id = %service.id,
        name = %service.name,
        outcome,
        kind = kind.as_str(),
        http = evidence.http_status.unwrap_or(0),
        latency_ms = evidence.latency_ms.unwrap_or(0),
        next = next_sec,
        "check"
    );
}

pub async fn supervise<F, Fut, H>(mut boot: F, mut on_dead: H)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
    H: FnMut(DateTime<Utc>, bool),
{
    let mut deaths = 0_u32;
    let mut started = std::time::Instant::now();
    loop {
        let handle = tokio::spawn(boot());
        match handle.await {
            Ok(()) => return,
            Err(err) if err.is_cancelled() => return,
            Err(_) => {
                let uptime = started.elapsed();
                deaths += 1;
                let restart = should_restart(deaths, uptime);
                on_dead(Utc::now(), restart);
                if !restart {
                    return;
                }
                if uptime >= WATCHDOG_RESET {
                    deaths = 1;
                }
                started = std::time::Instant::now();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ExpectedStatus, HeaderSpec, HttpMethod, MachineStatus, QuietHours, UiState,
    };
    use crate::notify::{NoopNotifier, Notification, Notifier, QueuedDown};
    use crate::store::{History, SecretStore};
    use std::sync::atomic::AtomicU32;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn at() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-18T14:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn sample(id: &str, url: String, interval: u32) -> Service {
        Service {
            id: id.into(),
            name: id.into(),
            url,
            method: HttpMethod::Get,
            headers: vec![],
            body: None,
            interval_sec: interval,
            timeout_ms: 5_000,
            expected_status: ExpectedStatus::TwoXx,
            assertions: vec![],
            max_latency_ms: None,
            action_url: None,
            notify: true,
            always_alert: false,
            paused: false,
            follow_redirects: true,
            fail_threshold: Some(1),
            group: None,
            created_at: at(),
            updated_at: at(),
        }
    }

    fn open_history() -> (tempfile::TempDir, History) {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path().join("history.sqlite3")).unwrap();
        (dir, history)
    }

    fn start(
        services: Vec<Service>,
        history: History,
        secrets: Arc<SecretStore>,
        events: Arc<dyn PulseEvents>,
    ) -> (SchedulerHandle, tokio::task::JoinHandle<()>) {
        let scheduler = Scheduler::new(SchedulerConfig {
            services,
            settings: AppSettings::default(),
            history,
            secrets,
            events,
            notifier: Box::new(NoopNotifier),
            enable_jitter: false,
            on_poller_dead: Arc::new(|_| {}),
        })
        .unwrap();
        let handle = scheduler.handle();
        let task = tokio::spawn(scheduler.run());
        (handle, task)
    }

    async fn wait_state(handle: &SchedulerHandle, id: &str, want: UiState) -> ServiceView {
        for _ in 0..200 {
            if let Ok(view) = handle.view(id) {
                if view.state == want {
                    return view;
                }
            }
            tokio::task::yield_now().await;
            sleep(Duration::from_millis(10)).await;
        }
        panic!("never reached {want:?}: {:?}", handle.view(id));
    }

    #[test]
    fn stagger_is_index_times_min_over_n_capped() {
        let interval = Duration::from_secs(60);
        assert_eq!(start_stagger(0, 4, interval), Duration::ZERO);
        assert_eq!(start_stagger(1, 4, interval), Duration::from_secs(15));
        assert_eq!(start_stagger(3, 4, interval), Duration::from_secs(15));
        assert_eq!(
            start_stagger(3, 4, Duration::from_secs(15)),
            Duration::from_secs_f64(11.25)
        );
        assert_eq!(start_stagger(1, 1, interval), Duration::ZERO);
        assert_eq!(
            start_stagger(9, 10, Duration::from_secs(30)),
            Duration::from_secs(15)
        );
    }

    #[test]
    fn jitter_stays_within_ten_percent() {
        let interval = Duration::from_secs(100);
        for seed in 0..64 {
            let got = with_jitter(interval, seed);
            assert!(got >= Duration::from_secs(90), "{got:?}");
            assert!(got <= Duration::from_secs(110), "{got:?}");
        }
    }

    #[test]
    fn watchdog_restarts_only_once() {
        assert!(should_restart(1, Duration::from_secs(1)));
        assert!(!should_restart(2, Duration::from_secs(1)));
        assert!(!should_restart(0, Duration::from_secs(1)));
        assert!(should_restart(2, WATCHDOG_RESET));
    }

    #[test]
    fn check_kind_is_snake_case() {
        assert_eq!(
            error_kind_name(ErrorKind::UnexpectedStatus),
            "unexpected_status"
        );
        assert_eq!(error_kind_name(ErrorKind::MissingSecret), "missing_secret");
    }

    #[tokio::test]
    async fn supervise_restarts_once_then_stays_dead() {
        let boots = Arc::new(AtomicU32::new(0));
        let deaths = Arc::new(AtomicU32::new(0));
        let boots_c = Arc::clone(&boots);
        let deaths_c = Arc::clone(&deaths);
        supervise(
            move || {
                let n = boots_c.fetch_add(1, Ordering::SeqCst);
                async move {
                    if n < 2 {
                        panic!("boom {n}");
                    }
                    std::future::pending::<()>().await;
                }
            },
            move |_, _| {
                deaths_c.fetch_add(1, Ordering::SeqCst);
            },
        )
        .await;
        assert_eq!(boots.load(Ordering::SeqCst), 2);
        assert_eq!(deaths.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn first_check_is_async_and_leaves_pending() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
            .mount(&server)
            .await;

        let (_dir, history) = open_history();
        let svc = sample("a", format!("{}/health", server.uri()), 15);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (dead_tx, _dead_rx) = tokio::sync::mpsc::unbounded_channel();
        let (handle, task) = start(
            vec![svc],
            history,
            Arc::new(SecretStore::for_test()),
            Arc::new(ChannelEvents {
                services: tx,
                dead: dead_tx,
            }),
        );

        let before = handle.view("a").unwrap();
        assert_eq!(before.state, UiState::Pending);
        assert!(before.last_result.is_none());

        let view = wait_state(&handle, "a", UiState::Healthy).await;
        assert_eq!(view.last_result.unwrap().evidence.http_status, Some(200));
        handle.with_history(|history| {
            let runtime = history.load_runtime("a").unwrap();
            assert!(runtime.last_check_at.is_some());
            assert_eq!(history.samples_24h("a", Utc::now()).unwrap().len(), 1);
        });

        handle.shutdown();
        let _ = task.await;
    }

    #[tokio::test(start_paused = true)]
    async fn save_upsert_is_pending_until_first_poll() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
            .mount(&server)
            .await;

        let (_dir, history) = open_history();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (dead_tx, _dead_rx) = tokio::sync::mpsc::unbounded_channel();
        let (handle, task) = start(
            vec![],
            history,
            Arc::new(SecretStore::for_test()),
            Arc::new(ChannelEvents {
                services: tx,
                dead: dead_tx,
            }),
        );

        handle.upsert(sample("new", format!("{}/health", server.uri()), 15));
        let before = handle.view("new").unwrap();
        assert_eq!(before.state, UiState::Pending);
        assert!(before.last_result.is_none());

        let view = wait_state(&handle, "new", UiState::Healthy).await;
        assert_eq!(view.last_result.unwrap().evidence.http_status, Some(200));

        handle.shutdown();
        let _ = task.await;
    }

    #[tokio::test(start_paused = true)]
    async fn start_stagger_delays_second_service() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/a"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/b"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let (_dir, history) = open_history();
        let a = sample("aaa", format!("{}/a", server.uri()), 15);
        let b = sample("bbb", format!("{}/b", server.uri()), 15);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (dead_tx, _dead_rx) = tokio::sync::mpsc::unbounded_channel();
        let (handle, task) = start(
            vec![a, b],
            history,
            Arc::new(SecretStore::for_test()),
            Arc::new(ChannelEvents {
                services: tx,
                dead: dead_tx,
            }),
        );

        wait_state(&handle, "aaa", UiState::Healthy).await;
        assert!(handle.view("bbb").unwrap().last_result.is_none());

        // i=1, n=2, min=15s → 7.5s
        tokio::time::advance(Duration::from_millis(7_400)).await;
        tokio::task::yield_now().await;
        assert!(handle.view("bbb").unwrap().last_result.is_none());

        tokio::time::advance(Duration::from_millis(200)).await;
        wait_state(&handle, "bbb", UiState::Healthy).await;

        handle.shutdown();
        let _ = task.await;
    }

    #[tokio::test(start_paused = true)]
    async fn pause_stops_polling_and_check_now_still_runs() {
        let server = MockServer::start().await;
        let hits = Arc::new(AtomicU32::new(0));
        let hits_c = Arc::clone(&hits);
        Mock::given(method("GET"))
            .respond_with(move |_req: &wiremock::Request| {
                hits_c.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200)
            })
            .mount(&server)
            .await;

        let (_dir, history) = open_history();
        let svc = sample("p", format!("{}/health", server.uri()), 15);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (dead_tx, _dead_rx) = tokio::sync::mpsc::unbounded_channel();
        let (handle, task) = start(
            vec![svc],
            history,
            Arc::new(SecretStore::for_test()),
            Arc::new(ChannelEvents {
                services: tx,
                dead: dead_tx,
            }),
        );

        wait_state(&handle, "p", UiState::Healthy).await;
        let view = handle.set_paused("p", true).unwrap();
        assert_eq!(view.state, UiState::Paused);
        let after_first = hits.load(Ordering::SeqCst);

        tokio::time::advance(Duration::from_secs(45)).await;
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert_eq!(hits.load(Ordering::SeqCst), after_first);

        let result = handle.check_now("p").await.unwrap();
        assert_eq!(result.evidence.http_status, Some(200));
        assert_eq!(hits.load(Ordering::SeqCst), after_first + 1);
        assert_eq!(handle.view("p").unwrap().state, UiState::Paused);

        handle.shutdown();
        let _ = task.await;
    }

    #[tokio::test]
    async fn pause_cancels_in_flight_check_now() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(400)))
            .mount(&server)
            .await;

        let (_dir, history) = open_history();
        let svc = sample("hang", format!("{}/slow", server.uri()), 60);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (dead_tx, _dead_rx) = tokio::sync::mpsc::unbounded_channel();
        let (handle, task) = start(
            vec![svc],
            history,
            Arc::new(SecretStore::for_test()),
            Arc::new(ChannelEvents {
                services: tx,
                dead: dead_tx,
            }),
        );

        wait_state(&handle, "hang", UiState::Healthy).await;
        let pending = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.check_now("hang").await })
        };
        tokio::time::sleep(Duration::from_millis(30)).await;
        handle.set_paused("hang", true).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(1), pending)
            .await
            .expect("check_now must not hang on pause")
            .unwrap();
        assert!(
            matches!(result, Err(SchedulerError::Canceled)),
            "expected Canceled, got {result:?}"
        );
        assert_eq!(handle.view("hang").unwrap().state, UiState::Paused);

        handle.shutdown();
        let _ = task.await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn missing_secret_is_hard_fail_without_http() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let (_dir, history) = open_history();
        let mut svc = sample("sec", format!("{}/health", server.uri()), 15);
        svc.headers.push(HeaderSpec {
            key: "Authorization".into(),
            secret: true,
            value: None,
        });
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (dead_tx, _dead_rx) = tokio::sync::mpsc::unbounded_channel();
        let scheduler = Scheduler::new(SchedulerConfig {
            services: vec![svc],
            settings: AppSettings::default(),
            history,
            secrets: Arc::new(SecretStore::for_test()),
            events: Arc::new(ChannelEvents {
                services: tx,
                dead: dead_tx,
            }),
            notifier: Box::new(NoopNotifier),
            enable_jitter: false,
            on_poller_dead: Arc::new(|_| {}),
        })
        .unwrap();
        let handle = scheduler.handle();
        // Do not start the loop — a live check must fail before HTTP.
        let result = handle.check_now("sec").await.unwrap();
        assert_eq!(result.evidence.error_kind, Some(ErrorKind::MissingSecret));
        assert_eq!(result.evidence.outcome, OutcomeClass::Hard);
        assert!(result
            .evidence
            .error
            .as_deref()
            .unwrap()
            .contains("Authorization"));
        drop(server);
    }

    #[tokio::test]
    async fn concurrency_caps_at_four() {
        let server = MockServer::start().await;
        let inflight = Arc::new(AtomicU32::new(0));
        let max = Arc::new(AtomicU32::new(0));
        let inflight_c = Arc::clone(&inflight);
        let max_c = Arc::clone(&max);
        Mock::given(method("GET"))
            .respond_with(move |_req: &wiremock::Request| {
                let now = inflight_c.fetch_add(1, Ordering::SeqCst) + 1;
                max_c.fetch_max(now, Ordering::SeqCst);
                let inflight = Arc::clone(&inflight_c);
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(80));
                    inflight.fetch_sub(1, Ordering::SeqCst);
                });
                ResponseTemplate::new(200).set_delay(Duration::from_millis(80))
            })
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path().join("history.sqlite3")).unwrap();
        let services: Vec<Service> = (0..6)
            .map(|i| sample(&format!("s{i}"), format!("{}/{i}", server.uri()), 60))
            .collect();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (dead_tx, _dead_rx) = tokio::sync::mpsc::unbounded_channel();
        let (handle, task) = start(
            services,
            history,
            Arc::new(SecretStore::for_test()),
            Arc::new(ChannelEvents {
                services: tx,
                dead: dead_tx,
            }),
        );

        // Let the first staggered task finish so check_all wakes a full set.
        tokio::time::sleep(Duration::from_millis(120)).await;
        let started = std::time::Instant::now();
        handle.check_all().await;
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(500),
            "check_all must not await HTTP: {elapsed:?}"
        );
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert!(max.load(Ordering::SeqCst) <= CONCURRENCY as u32);
        assert!(max.load(Ordering::SeqCst) >= 2);

        handle.shutdown();
        let _ = task.await;
    }

    #[tokio::test(start_paused = true)]
    async fn check_now_does_not_double_probe() {
        let server = MockServer::start().await;
        let hits = Arc::new(AtomicU32::new(0));
        let hits_c = Arc::clone(&hits);
        Mock::given(method("GET"))
            .respond_with(move |_req: &wiremock::Request| {
                hits_c.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200)
            })
            .mount(&server)
            .await;

        let (_dir, history) = open_history();
        let svc = sample("once", format!("{}/health", server.uri()), 15);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (dead_tx, _dead_rx) = tokio::sync::mpsc::unbounded_channel();
        let (handle, task) = start(
            vec![svc],
            history,
            Arc::new(SecretStore::for_test()),
            Arc::new(ChannelEvents {
                services: tx,
                dead: dead_tx,
            }),
        );

        wait_state(&handle, "once", UiState::Healthy).await;
        let after_first = hits.load(Ordering::SeqCst);
        handle.check_now("once").await.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), after_first + 1);
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(hits.load(Ordering::SeqCst), after_first + 1);

        handle.shutdown();
        let _ = task.await;
    }

    #[tokio::test]
    async fn down_grouper_flushes_after_two_seconds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let events = Arc::new(Mutex::new(Vec::new()));
        struct Capture(Arc<Mutex<Vec<Notification>>>);
        impl Notifier for Capture {
            fn notify(&mut self, notification: Notification) {
                self.0.lock().expect("notify lock").push(notification);
            }
        }

        let (_dir, history) = open_history();
        let svc = sample("down", format!("{}/health", server.uri()), 60);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (dead_tx, _dead_rx) = tokio::sync::mpsc::unbounded_channel();
        let scheduler = Scheduler::new(SchedulerConfig {
            services: vec![svc],
            settings: AppSettings::default(),
            history,
            secrets: Arc::new(SecretStore::for_test()),
            events: Arc::new(ChannelEvents {
                services: tx,
                dead: dead_tx,
            }),
            notifier: Box::new(Capture(Arc::clone(&events))),
            enable_jitter: false,
            on_poller_dead: Arc::new(|_| {}),
        })
        .unwrap();
        let handle = scheduler.handle();
        let task = tokio::spawn(scheduler.run());

        wait_state(&handle, "down", UiState::Down).await;
        assert!(
            events.lock().expect("events").is_empty(),
            "Down must wait the 2s group window"
        );

        // Grouper uses wall-clock Utc::now(); do not use start_paused here.
        tokio::time::sleep(Duration::from_millis(2_100)).await;
        let got = events.lock().expect("events").clone();
        assert!(
            got.iter().any(
                |n| matches!(n, Notification::Down { service_id, .. } if service_id == "down")
            ),
            "expected Down after 2s, got {got:?}"
        );

        handle.shutdown();
        let _ = task.await;
    }

    struct OfflineCap {
        services: tokio::sync::mpsc::UnboundedSender<Vec<ServiceView>>,
        dead: tokio::sync::mpsc::UnboundedSender<DateTime<Utc>>,
        offline: tokio::sync::mpsc::UnboundedSender<bool>,
    }

    impl PulseEvents for OfflineCap {
        fn emit_services(&self, views: &[ServiceView]) {
            let _ = self.services.send(views.to_vec());
        }
        fn emit_poller_dead(&self, at: DateTime<Utc>) {
            let _ = self.dead.send(at);
        }
        fn emit_offline(&self, offline: bool) {
            let _ = self.offline.send(offline);
        }
    }

    #[tokio::test]
    async fn two_dns_hosts_freeze_without_samples() {
        let (_dir, history) = open_history();
        let a = sample("aaa", "http://aaa.invalid/health".into(), 15);
        let b = sample("bbb", "http://bbb.invalid/health".into(), 15);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (dead_tx, _dead_rx) = tokio::sync::mpsc::unbounded_channel();
        let (off_tx, mut off_rx) = tokio::sync::mpsc::unbounded_channel();
        let (handle, task) = {
            let scheduler = Scheduler::new(SchedulerConfig {
                services: vec![a, b],
                settings: AppSettings::default(),
                history,
                secrets: Arc::new(SecretStore::for_test()),
                events: Arc::new(OfflineCap {
                    services: tx,
                    dead: dead_tx,
                    offline: off_tx,
                }),
                notifier: Box::new(NoopNotifier),
                enable_jitter: false,
                on_poller_dead: Arc::new(|_| {}),
            })
            .unwrap();
            let handle = scheduler.handle();
            (handle.clone(), tokio::spawn(scheduler.run()))
        };

        let entered = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if off_rx.recv().await == Some(true) {
                    return;
                }
            }
        })
        .await;
        assert!(entered.is_ok(), "expected offline enter");
        assert!(handle.is_offline());

        handle.with_history(|history| {
            let a_samples = history.samples_24h("aaa", Utc::now()).unwrap().len();
            let b_samples = history.samples_24h("bbb", Utc::now()).unwrap().len();
            // The first host fail applies; the second enters offline and is frozen.
            assert!(
                a_samples + b_samples <= 1,
                "offline-frozen probes are not sampled"
            );
        });

        tokio::time::sleep(Duration::from_millis(80)).await;
        handle.with_history(|history| {
            let total = history.samples_24h("aaa", Utc::now()).unwrap().len()
                + history.samples_24h("bbb", Utc::now()).unwrap().len();
            assert!(total <= 1);
            let a_fails = history.load_runtime("aaa").unwrap().consecutive_hard_fails;
            let b_fails = history.load_runtime("bbb").unwrap().consecutive_hard_fails;
            assert!(a_fails + b_fails <= 1, "fail counters freeze while offline");
        });

        handle.shutdown();
        let _ = task.await;
    }

    #[tokio::test]
    async fn any_success_exits_offline() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let (_dir, history) = open_history();
        let a = sample("aaa", "http://aaa.invalid/health".into(), 15);
        let b = sample("bbb", "http://bbb.invalid/health".into(), 15);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (dead_tx, _dead_rx) = tokio::sync::mpsc::unbounded_channel();
        let (off_tx, mut off_rx) = tokio::sync::mpsc::unbounded_channel();
        let scheduler = Scheduler::new(SchedulerConfig {
            services: vec![a, b],
            settings: AppSettings::default(),
            history,
            secrets: Arc::new(SecretStore::for_test()),
            events: Arc::new(OfflineCap {
                services: tx,
                dead: dead_tx,
                offline: off_tx,
            }),
            notifier: Box::new(NoopNotifier),
            enable_jitter: false,
            on_poller_dead: Arc::new(|_| {}),
        })
        .unwrap();
        let handle = scheduler.handle();
        let task = tokio::spawn(scheduler.run());

        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if off_rx.recv().await == Some(true) {
                    return;
                }
            }
        })
        .await
        .expect("enter offline");

        let mut up = sample("ccc", format!("{}/ok", server.uri()), 15);
        up.fail_threshold = Some(1);
        handle.upsert(up);
        let result = handle.check_now("ccc").await.unwrap();
        assert_eq!(result.evidence.http_status, Some(200));
        assert!(!handle.is_offline());

        handle.shutdown();
        let _ = task.await;
    }

    #[tokio::test]
    async fn single_host_timeout_is_a_normal_hard_fail() {
        let (_dir, history) = open_history();
        let mut svc = sample("only", "http://only.invalid/health".into(), 15);
        svc.fail_threshold = Some(1);
        svc.timeout_ms = 500;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (dead_tx, _dead_rx) = tokio::sync::mpsc::unbounded_channel();
        let (handle, task) = start(
            vec![svc],
            history,
            Arc::new(SecretStore::for_test()),
            Arc::new(ChannelEvents {
                services: tx,
                dead: dead_tx,
            }),
        );

        wait_state(&handle, "only", UiState::Down).await;
        assert!(!handle.is_offline());
        handle.with_history(|history| {
            assert_eq!(history.samples_24h("only", Utc::now()).unwrap().len(), 1);
            assert_eq!(
                history.load_runtime("only").unwrap().consecutive_hard_fails,
                1
            );
        });

        handle.shutdown();
        let _ = task.await;
    }

    #[tokio::test]
    async fn sleep_wake_persists_slept_at_and_settles() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let (_dir, history) = open_history();
        let mut svc = sample("down", format!("{}/health", server.uri()), 60);
        svc.fail_threshold = Some(1);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (dead_tx, _dead_rx) = tokio::sync::mpsc::unbounded_channel();
        let (handle, task) = start(
            vec![svc],
            history,
            Arc::new(SecretStore::for_test()),
            Arc::new(ChannelEvents {
                services: tx,
                dead: dead_tx,
            }),
        );

        wait_state(&handle, "down", UiState::Down).await;
        handle.on_os_sleep();
        handle.with_history(|history| {
            assert!(history.load_runtime("down").unwrap().slept_at.is_some());
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let resume = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.resume_from_wake().await })
        };
        tokio::time::sleep(Duration::from_millis(2_200)).await;
        resume.await.unwrap();
        handle.with_history(|history| {
            let state = history.load_runtime("down").unwrap();
            assert!(state.slept_at.is_none());
            assert!(state.down_clock_adjust_ms > 0);
        });

        handle.shutdown();
        let _ = task.await;
    }

    #[tokio::test]
    async fn wake_grace_skips_transport_fail_counters() {
        let (_dir, history) = open_history();
        let mut svc = sample("g", "http://grace.invalid/health".into(), 60);
        svc.fail_threshold = Some(1);
        svc.timeout_ms = 400;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (dead_tx, _dead_rx) = tokio::sync::mpsc::unbounded_channel();
        let scheduler = Scheduler::new(SchedulerConfig {
            services: vec![svc],
            settings: AppSettings::default(),
            history,
            secrets: Arc::new(SecretStore::for_test()),
            events: Arc::new(ChannelEvents {
                services: tx,
                dead: dead_tx,
            }),
            notifier: Box::new(NoopNotifier),
            enable_jitter: false,
            on_poller_dead: Arc::new(|_| {}),
        })
        .unwrap();
        let handle = scheduler.handle();
        handle.on_os_sleep();
        let resume = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.resume_from_wake().await })
        };
        tokio::time::sleep(Duration::from_millis(2_200)).await;
        resume.await.unwrap();

        let result = handle.check_now("g").await.unwrap();
        assert_eq!(result.evidence.error_kind, Some(ErrorKind::Dns));
        handle.with_history(|history| {
            assert_eq!(history.load_runtime("g").unwrap().consecutive_hard_fails, 0);
            assert!(history.samples_24h("g", Utc::now()).unwrap().is_empty());
        });
        handle.shutdown();
    }

    #[tokio::test]
    async fn startup_folds_leftover_slept_at() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500).set_delay(Duration::from_secs(8)))
            .mount(&server)
            .await;

        let (_dir, history) = open_history();
        let sleep_at = Utc::now() - chrono::Duration::minutes(5);
        let mut state = RuntimeState::pending();
        state.status = MachineStatus::Down;
        state.consecutive_hard_fails = 3;
        state.down_since = Some(sleep_at - chrono::Duration::minutes(5));
        state.slept_at = Some(sleep_at);
        history.put_runtime("a", &state).unwrap();

        let svc = sample("a", format!("{}/health", server.uri()), 60);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (dead_tx, _dead_rx) = tokio::sync::mpsc::unbounded_channel();
        let (handle, task) = start(
            vec![svc],
            history,
            Arc::new(SecretStore::for_test()),
            Arc::new(ChannelEvents {
                services: tx,
                dead: dead_tx,
            }),
        );

        let folded = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if handle.with_history(|history| {
                    history
                        .load_runtime("a")
                        .ok()
                        .is_some_and(|runtime| runtime.slept_at.is_none())
                }) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(folded.is_ok(), "startup must fold leftover slept_at");
        handle.with_history(|history| {
            let runtime = history.load_runtime("a").unwrap();
            assert!(runtime.down_clock_adjust_ms >= 4 * 60 * 1000);
        });

        handle.shutdown();
        let _ = task.await;
    }

    #[tokio::test]
    async fn grace_transport_does_not_enter_offline() {
        let (_dir, history) = open_history();
        let a = sample("aaa", "http://aaa.invalid/health".into(), 60);
        let b = sample("bbb", "http://bbb.invalid/health".into(), 60);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (dead_tx, _dead_rx) = tokio::sync::mpsc::unbounded_channel();
        let scheduler = Scheduler::new(SchedulerConfig {
            services: vec![a, b],
            settings: AppSettings::default(),
            history,
            secrets: Arc::new(SecretStore::for_test()),
            events: Arc::new(ChannelEvents {
                services: tx,
                dead: dead_tx,
            }),
            notifier: Box::new(NoopNotifier),
            enable_jitter: false,
            on_poller_dead: Arc::new(|_| {}),
        })
        .unwrap();
        let handle = scheduler.handle();
        let resume = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.resume_from_wake().await })
        };
        tokio::time::sleep(Duration::from_millis(2_200)).await;
        resume.await.unwrap();

        let _ = handle.check_now("aaa").await;
        let _ = handle.check_now("bbb").await;
        assert!(
            !handle.is_offline(),
            "grace-window transport fails must not enter offline"
        );
        handle.shutdown();
    }

    struct Capture(Arc<Mutex<Vec<Notification>>>);
    impl Notifier for Capture {
        fn notify(&mut self, notification: Notification) {
            self.0.lock().expect("notify lock").push(notification);
        }
    }

    fn queued(id: &str, name: &str) -> QueuedDown {
        QueuedDown {
            service_id: id.into(),
            name: name.into(),
            title: name.into(),
            body: "HTTP 502 · 1.4s".into(),
        }
    }

    fn down_runtime() -> RuntimeState {
        let mut runtime = RuntimeState::pending();
        runtime.status = MachineStatus::Down;
        runtime
    }

    fn always_quiet() -> AppSettings {
        AppSettings {
            quiet_hours: Some(QuietHours {
                start: "00:00".into(),
                end: "00:00".into(),
                days: vec![0, 1, 2, 3, 4, 5, 6],
            }),
            ..AppSettings::default()
        }
    }

    fn handle_with_capture(
        services: Vec<Service>,
        history: History,
    ) -> (SchedulerHandle, Arc<Mutex<Vec<Notification>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (dead_tx, _dead_rx) = tokio::sync::mpsc::unbounded_channel();
        let scheduler = Scheduler::new(SchedulerConfig {
            services,
            settings: AppSettings::default(),
            history,
            secrets: Arc::new(SecretStore::for_test()),
            events: Arc::new(ChannelEvents {
                services: tx,
                dead: dead_tx,
            }),
            notifier: Box::new(Capture(Arc::clone(&events))),
            enable_jitter: false,
            on_poller_dead: Arc::new(|_| {}),
        })
        .unwrap();
        (scheduler.handle(), events)
    }

    #[test]
    fn flush_digest_is_queue_not_worst_of() {
        let (_dir, history) = open_history();
        let a = sample("pay", "https://pay.example/health".into(), 60);
        let b = sample("worker", "https://worker.example/health".into(), 60);
        let (handle, events) = handle_with_capture(vec![a, b], history);
        handle.with_history(|history| {
            history.put_runtime("pay", &down_runtime()).unwrap();
            history.put_runtime("worker", &down_runtime()).unwrap();
        });
        {
            let mut queue = handle.inner.quiet.lock().expect("quiet lock");
            queue.enter(queued("pay", "Payments"));
            queue.enter(queued("worker", "Worker"));
        }
        handle.inner.maybe_flush_quiet(Utc::now());
        let got = events.lock().expect("events").clone();
        assert_eq!(got.len(), 1);
        match &got[0] {
            Notification::Digest {
                service_ids, title, ..
            } => {
                assert_eq!(service_ids, &["pay", "worker"]);
                assert_eq!(title, "2 services down");
            }
            other => panic!("expected digest, got {other:?}"),
        }
        assert!(handle.inner.quiet.lock().expect("quiet lock").is_empty());
    }

    #[test]
    fn flush_drops_snoozed_and_recovered() {
        let (_dir, history) = open_history();
        let a = sample("pay", "https://pay.example/health".into(), 60);
        let b = sample("worker", "https://worker.example/health".into(), 60);
        let c = sample("auth", "https://auth.example/health".into(), 60);
        let (handle, events) = handle_with_capture(vec![a, b, c], history);
        let now = Utc::now();
        handle.with_history(|history| {
            let mut snoozed = down_runtime();
            snoozed.snooze_until = Some(now + chrono::Duration::hours(1));
            history.put_runtime("pay", &snoozed).unwrap();
            history.put_runtime("worker", &down_runtime()).unwrap();
            history
                .put_runtime("auth", &RuntimeState::pending())
                .unwrap();
        });
        {
            let mut queue = handle.inner.quiet.lock().expect("quiet lock");
            queue.enter(queued("pay", "Payments"));
            queue.enter(queued("worker", "Worker"));
            queue.enter(queued("auth", "Auth"));
        }
        handle.inner.maybe_flush_quiet(now);
        let got = events.lock().expect("events").clone();
        assert_eq!(
            got,
            vec![Notification::Down {
                service_id: "worker".into(),
                title: "Worker".into(),
                body: "HTTP 502 · 1.4s".into(),
            }]
        );
    }

    #[test]
    fn quiet_window_holds_queue_until_settings_clear() {
        let (_dir, history) = open_history();
        let a = sample("pay", "https://pay.example/health".into(), 60);
        let b = sample("worker", "https://worker.example/health".into(), 60);
        let (handle, events) = handle_with_capture(vec![a, b], history);
        handle.update_settings(always_quiet());
        handle.with_history(|history| {
            history.put_runtime("pay", &down_runtime()).unwrap();
            history.put_runtime("worker", &down_runtime()).unwrap();
        });
        {
            let mut queue = handle.inner.quiet.lock().expect("quiet lock");
            queue.enter(queued("pay", "Payments"));
            queue.enter(queued("worker", "Worker"));
        }
        handle.inner.maybe_flush_quiet(Utc::now());
        assert!(events.lock().expect("events").is_empty());
        assert_eq!(handle.inner.quiet.lock().expect("quiet lock").len(), 2);

        handle.update_settings(AppSettings::default());
        let got = events.lock().expect("events").clone();
        assert!(
            matches!(got.as_slice(), [Notification::Digest { .. }]),
            "clearing quiet hours flushes the digest, got {got:?}"
        );
    }

    #[test]
    fn snooze_drops_from_queue_and_writes_sqlite_only() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path().join("history.sqlite3")).unwrap();
        let svc = sample("pay", "https://pay.example/health".into(), 60);
        let store = crate::store::ConfigStore::open(crate::store::Paths::new(dir.path())).unwrap();
        store.save_services(std::slice::from_ref(&svc)).unwrap();
        let (handle, _events) = handle_with_capture(vec![svc], history);
        handle
            .inner
            .quiet
            .lock()
            .expect("quiet lock")
            .enter(queued("pay", "Payments"));
        let until = DateTime::from_timestamp_millis(
            (Utc::now() + chrono::Duration::minutes(60)).timestamp_millis(),
        )
        .unwrap();
        handle.set_snooze("pay", Some(until)).unwrap();
        assert!(!handle
            .inner
            .quiet
            .lock()
            .expect("quiet lock")
            .contains("pay"));
        handle.with_history(|history| {
            assert_eq!(
                history.load_runtime("pay").unwrap().snooze_until,
                Some(until)
            );
        });
        let services = store.load_services().unwrap();
        let encoded = serde_json::to_value(&services[0]).unwrap();
        assert!(encoded.get("snoozeUntil").is_none());
        let view = handle.view("pay").unwrap();
        assert_eq!(view.snooze_until, Some(until));
        assert_eq!(view.state, UiState::Pending);
    }

    #[test]
    fn recovery_after_window_cancels_held_down() {
        let (_dir, history) = open_history();
        let a = sample("pay", "https://pay.example/health".into(), 60);
        let (handle, events) = handle_with_capture(vec![a], history);
        handle
            .inner
            .quiet
            .lock()
            .expect("quiet lock")
            .enter(queued("pay", "Payments"));
        // Recovered after the window: still in the queue, runtime no longer Down.
        handle.with_history(|history| {
            history
                .put_runtime("pay", &RuntimeState::pending())
                .unwrap();
        });
        handle.inner.maybe_flush_quiet(Utc::now());
        assert!(events.lock().expect("events").is_empty());
        assert!(handle.inner.quiet.lock().expect("quiet lock").is_empty());
    }

    #[test]
    fn flush_is_noop_when_notifications_disabled() {
        let (_dir, history) = open_history();
        let a = sample("pay", "https://pay.example/health".into(), 60);
        let b = sample("worker", "https://worker.example/health".into(), 60);
        let (handle, events) = handle_with_capture(vec![a, b], history);
        handle.with_history(|history| {
            history.put_runtime("pay", &down_runtime()).unwrap();
            history.put_runtime("worker", &down_runtime()).unwrap();
        });
        {
            let mut queue = handle.inner.quiet.lock().expect("quiet lock");
            queue.enter(queued("pay", "Payments"));
            queue.enter(queued("worker", "Worker"));
        }
        handle.update_settings(AppSettings {
            notifications: false,
            ..AppSettings::default()
        });
        assert!(events.lock().expect("events").is_empty());
        assert!(handle.inner.quiet.lock().expect("quiet lock").is_empty());
    }
}
