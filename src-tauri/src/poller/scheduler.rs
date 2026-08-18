use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{Notify, Semaphore};
use tokio::task::AbortHandle;
use tokio::time::sleep;

use crate::domain::view::{assemble_view, compact_sample};
use crate::domain::{
    AppSettings, CheckEvidence, CheckResult, ErrorKind, MessageArgs, OutcomeClass, RuntimeState,
    Service, ServiceStatus, ServiceView,
};
use crate::eval::{evaluate_at, outcome_of};
use crate::notify::{
    in_quiet_hours, DownGrouper, Emit, Notification, Notifier, NotifyPolicy, QueueOp, QueuedDown,
    QuietQueue,
};
use crate::poller::state_machine::{fail_threshold, on_result, ProbeEvent};
use crate::poller::HttpClient;
use crate::store::{History, MissingSecret, SecretStore, StoreError};

pub const CONCURRENCY: usize = 4;
pub const STAGGER_CAP: Duration = Duration::from_secs(15);
pub const CHECK_ALL_GAP: Duration = Duration::from_millis(50);
pub const VIEW_COALESCE: Duration = Duration::from_millis(100);
pub const JITTER_FRAC: f64 = 0.10;

pub trait PulseEvents: Send + Sync {
    fn emit_services(&self, views: &[ServiceView]);
    fn emit_poller_dead(&self, at: DateTime<Utc>);
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
}

#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("service not found")]
    NotFound,
    #[error("check canceled")]
    Canceled,
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

/// One restart. A second death stays in `poller_dead` and does not loop.
pub fn should_restart(death_count: u32) -> bool {
    death_count == 1
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
    }

    pub fn upsert(&self, service: Service) {
        self.inner.upsert(service);
        self.inner.mark_dirty();
    }

    pub fn remove(&self, id: &str) {
        self.inner.abort_one(id);
        self.inner.slots.lock().expect("slots lock").remove(id);
        self.inner.mark_dirty();
    }

    pub fn set_paused(&self, id: &str, paused: bool) -> Result<ServiceView, SchedulerError> {
        self.inner.set_paused(id, paused)?;
        self.inner.mark_dirty();
        self.view(id)
    }

    pub async fn check_now(&self, id: &str) -> Result<CheckResult, SchedulerError> {
        let service = self
            .inner
            .clone_service(id)
            .ok_or(SchedulerError::NotFound)?;
        self.inner.wake(id);
        self.inner.run_check(&service).await
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
            if let Some(service) = self.inner.clone_service(&id) {
                self.inner.wake(&id);
                let _ = self.inner.run_check(&service).await;
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
}

impl Inner {
    fn wake(&self, id: &str) {
        if let Some(slot) = self.slots.lock().expect("slots lock").get(id) {
            slot.check_now.notify_one();
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
        }
    }

    fn abort_all(&self) {
        for slot in self.slots.lock().expect("slots lock").values_mut() {
            if let Some(abort) = slot.abort.take() {
                abort.abort();
            }
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
            } else {
                slots.insert(
                    id.clone(),
                    Slot {
                        service,
                        check_now: Arc::new(Notify::new()),
                        abort: None,
                    },
                );
            }
        }
        if !paused && !self.stopped.load(Ordering::SeqCst) {
            let delay = self.stagger_for(&id);
            self.spawn_service(id, delay);
        }
    }

    fn set_paused(self: &Arc<Self>, id: &str, paused: bool) -> Result<(), SchedulerError> {
        let now = Utc::now();
        {
            let mut slots = self.slots.lock().expect("slots lock");
            let slot = slots.get_mut(id).ok_or(SchedulerError::NotFound)?;
            slot.service.paused = paused;
            if paused {
                if let Some(abort) = slot.abort.take() {
                    abort.abort();
                }
            }
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

    async fn supervise(self: Arc<Self>) {
        self.stopped.store(false, Ordering::SeqCst);
        self.poller_dead.store(false, Ordering::SeqCst);
        (self.on_poller_dead)(false);
        self.spawn_all();
        let coalesce = {
            let inner = Arc::clone(&self);
            tokio::spawn(async move { inner.coalesce_loop().await })
        };
        let prune = {
            let inner = Arc::clone(&self);
            tokio::spawn(async move { inner.prune_loop().await })
        };
        self.mark_dirty();
        self.cancelled().await;
        coalesce.abort();
        prune.abort();
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
            let _ = self.run_check(&service).await;
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
            tokio::select! {
                _ = sleep(wait) => {}
                _ = check_now.notified() => {}
                _ = self.cancelled() => return,
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
                missing_secret_evidence(&missing, now),
                missing.identity_changed || self.secrets.service_identity_changed(&service.id),
            ),
        };
        drop(permit);

        let settings = self.settings.read().expect("settings lock").clone();
        let mut runtime = {
            let history = self.history.lock().expect("history lock");
            history
                .load_runtime(&service.id)
                .unwrap_or_else(|_| RuntimeState::pending())
        };
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
        let threshold = fail_threshold(service.fail_threshold, settings.fail_threshold);
        let paused = self
            .clone_service(&service.id)
            .map(|current| current.paused)
            .unwrap_or(service.paused);
        // Offline detector is PR 9 — pass false; History still skips canceled/offline.
        let transition = on_result(
            &mut runtime,
            ProbeEvent::Applied(outcome_of(&evidence)),
            now,
            threshold,
            paused,
            false,
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

        if transition.applied {
            let sample = compact_sample(&result);
            let history = self.history.lock().expect("history lock");
            history.put_runtime(&service.id, &runtime)?;
            history.put_last_result(&service.id, &result)?;
            history.insert_sample(&service.id, &sample)?;
        }

        if let Some(emit) = transition.emit {
            self.emit_notification(service, &evidence, emit, now);
        }
        if transition.queue != QueueOp::None {
            self.quiet.lock().expect("quiet lock").apply(
                transition.queue,
                QueuedDown {
                    service_id: service.id.clone(),
                    name: service.name.clone(),
                    title: service.name.clone(),
                    body: String::new(),
                },
            );
        }

        self.checks.fetch_add(1, Ordering::Relaxed);
        log_check(service, &evidence, service.interval_sec);
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
            let ready = self.grouper.lock().expect("grouper lock").poll(Utc::now());
            if !ready.is_empty() {
                let mut notifier = self.notifier.lock().expect("notifier lock");
                for item in ready {
                    notifier.notify(item);
                }
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

fn fnv(id: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn missing_secret_evidence(missing: &MissingSecret, at: DateTime<Utc>) -> CheckEvidence {
    CheckEvidence {
        at,
        outcome: OutcomeClass::Hard,
        http_status: None,
        latency_ms: None,
        redirects: None,
        headers_stripped_on_redirect: None,
        assertion_results: Vec::new(),
        assertion_skipped: None,
        error_kind: Some(ErrorKind::MissingSecret),
        error: Some(ErrorKind::MissingSecret.user_message(&MessageArgs {
            secret_key: Some(&missing.key),
            ..MessageArgs::default()
        })),
        body_preview: None,
    }
}

fn log_check(service: &Service, evidence: &CheckEvidence, next_sec: u32) {
    let outcome = match evidence.outcome {
        OutcomeClass::Ok => "ok",
        OutcomeClass::Soft => "soft_fail",
        OutcomeClass::Hard => "hard_fail",
    };
    let kind = evidence
        .error_kind
        .map(|kind| format!("{kind:?}"))
        .unwrap_or_default();
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
    loop {
        let handle = tokio::spawn(boot());
        match handle.await {
            Ok(()) => return,
            Err(err) if err.is_cancelled() => return,
            Err(_) => {
                deaths += 1;
                let restart = should_restart(deaths);
                on_dead(Utc::now(), restart);
                if !restart {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ExpectedStatus, HeaderSpec, HttpMethod, UiState};
    use crate::notify::NoopNotifier;
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
        assert!(should_restart(1));
        assert!(!should_restart(2));
        assert!(!should_restart(0));
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
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(80)))
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

        let started = std::time::Instant::now();
        handle.check_all().await;
        let elapsed = started.elapsed();
        // 6 checks at 80ms with cap 4 cannot finish in one wave.
        assert!(elapsed >= Duration::from_millis(80));

        handle.shutdown();
        let _ = task.await;
    }
}
