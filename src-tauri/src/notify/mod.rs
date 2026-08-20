mod copy;
#[cfg(target_os = "macos")]
mod macos;
pub mod os;
mod quiet;

pub use copy::{
    digest_body, digest_title, down_body, down_title, format_duration, format_latency,
    recovered_body, recovered_title, DIGEST_BODY_MAX,
};
pub use os::{
    handle_activation, last_notified_service_id, parse_focus_args,
    request_permission_on_notify_save, NotifyHub, OsNotifier, FOCUS_LAUNCH,
};
pub use quiet::{
    flush_quiet_queue, in_quiet_hours, in_quiet_window, QueueOp, QueuedDown, QuietQueue,
};

use chrono::{DateTime, Utc};

use crate::domain::{CheckEvidence, RuntimeState};

/// Headless sink. `OsNotifier` is the desktop implementation.
pub trait Notifier {
    fn notify(&mut self, notification: Notification);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notification {
    Down {
        service_id: String,
        title: String,
        body: String,
    },
    Recovered {
        service_id: String,
        title: String,
        body: String,
    },
    Digest {
        service_ids: Vec<String>,
        title: String,
        body: String,
    },
}

impl Notification {
    pub fn down(
        service_id: impl Into<String>,
        name: &str,
        evidence: &CheckEvidence,
        timeout_ms: u32,
    ) -> Self {
        Self::Down {
            service_id: service_id.into(),
            title: down_title(name),
            body: down_body(evidence, timeout_ms),
        }
    }

    pub fn recovered(service_id: impl Into<String>, name: &str, duration_ms: u64) -> Self {
        Self::Recovered {
            service_id: service_id.into(),
            title: recovered_title(name),
            body: recovered_body(duration_ms),
        }
    }

    pub fn digest(names: &[(&str, &str)]) -> Self {
        let service_ids = names.iter().map(|(id, _)| (*id).to_string()).collect();
        let just_names: Vec<&str> = names.iter().map(|(_, name)| *name).collect();
        Self::Digest {
            service_ids,
            title: digest_title(names.len()),
            body: digest_body(&just_names),
        }
    }

    pub fn service_id(&self) -> Option<&str> {
        match self {
            Self::Down { service_id, .. } | Self::Recovered { service_id, .. } => Some(service_id),
            Self::Digest { .. } => None,
        }
    }
}

/// Records events. No OS toasts.
#[derive(Debug, Default, Clone)]
pub struct RecordingNotifier {
    pub events: Vec<Notification>,
}

impl Notifier for RecordingNotifier {
    fn notify(&mut self, notification: Notification) {
        self.events.push(notification);
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopNotifier;

impl Notifier for NoopNotifier {
    fn notify(&mut self, _notification: Notification) {}
}

/// Inputs to `notify_enabled()` / quiet-hours enqueue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyPolicy {
    pub notifications: bool,
    pub service_notify: bool,
    pub always_alert: bool,
    pub in_quiet_hours: bool,
    pub snoozed: bool,
    pub keychain_identity_changed: bool,
}

impl Default for NotifyPolicy {
    fn default() -> Self {
        Self {
            notifications: true,
            service_notify: true,
            always_alert: false,
            in_quiet_hours: false,
            snoozed: false,
            keychain_identity_changed: false,
        }
    }
}

impl NotifyPolicy {
    pub fn with_runtime(mut self, runtime: &RuntimeState, now: DateTime<Utc>) -> Self {
        self.snoozed = runtime.is_snoozed(now);
        self
    }

    /// `settings.notifications && service.notify && (alwaysAlert || !quiet) && !snoozed && !keychainIdentityChanged`
    pub fn notify_enabled(&self) -> bool {
        self.notifications
            && self.service_notify
            && (self.always_alert || !self.in_quiet_hours)
            && !self.snoozed
            && !self.keychain_identity_changed
    }

    /// Would have toasted if not for the quiet window. Snooze / identity miss never enqueue.
    pub fn should_queue(&self) -> bool {
        self.notifications
            && self.service_notify
            && self.in_quiet_hours
            && !self.always_alert
            && !self.snoozed
            && !self.keychain_identity_changed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emit {
    Down,
    Recovered { duration_ms: u64 },
}

pub const DOWN_GROUP_WINDOW_MS: i64 = 2_000;

/// Holds immediate Down toasts for 2s so a burst collapses to a digest.
#[derive(Debug, Default)]
pub struct DownGrouper {
    pending: Vec<Notification>,
    window_start: Option<DateTime<Utc>>,
}

impl DownGrouper {
    pub fn new() -> Self {
        Self::default()
    }

    /// Buffer a Down, or pass Recovered/Digest through. Returns events ready now.
    pub fn push(&mut self, notification: Notification, now: DateTime<Utc>) -> Vec<Notification> {
        match notification {
            Notification::Down { .. } => {
                let mut ready = Vec::new();
                if let Some(start) = self.window_start {
                    if (now - start).num_milliseconds() > DOWN_GROUP_WINDOW_MS {
                        ready.extend(self.take_ready());
                    }
                }
                if self.window_start.is_none() {
                    self.window_start = Some(now);
                }
                self.pending.push(notification);
                ready
            }
            Notification::Recovered { ref service_id, .. } => {
                // Drop a not-yet-emitted Down for the same service so Recovered is not followed by Down.
                let id = service_id.clone();
                self.pending.retain(|pending| match pending {
                    Notification::Down { service_id, .. } => service_id != &id,
                    _ => true,
                });
                if self.pending.is_empty() {
                    self.window_start = None;
                }
                vec![notification]
            }
            Notification::Digest { .. } => vec![notification],
        }
    }

    pub fn poll(&mut self, now: DateTime<Utc>) -> Vec<Notification> {
        match self.window_start {
            Some(start) if (now - start).num_milliseconds() >= DOWN_GROUP_WINDOW_MS => {
                self.take_ready()
            }
            _ => Vec::new(),
        }
    }

    fn take_ready(&mut self) -> Vec<Notification> {
        self.window_start = None;
        let pending = std::mem::take(&mut self.pending);
        match pending.len() {
            0 => Vec::new(),
            1 => pending,
            n => {
                let pairs: Vec<(String, String)> = pending
                    .into_iter()
                    .filter_map(|event| match event {
                        Notification::Down {
                            service_id, title, ..
                        } => Some((service_id, title)),
                        _ => None,
                    })
                    .collect();
                let refs: Vec<(&str, &str)> = pairs
                    .iter()
                    .map(|(id, name)| (id.as_str(), name.as_str()))
                    .collect();
                debug_assert_eq!(refs.len(), n);
                vec![Notification::digest(&refs)]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ErrorKind, OutcomeClass};

    fn t(ms: i64) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(1_700_000_000_000 + ms).unwrap()
    }

    fn down(id: &str) -> Notification {
        Notification::Down {
            service_id: id.into(),
            title: id.into(),
            body: "HTTP 502 · 1.4s".into(),
        }
    }

    fn recovered(id: &str) -> Notification {
        Notification::Recovered {
            service_id: id.into(),
            title: id.into(),
            body: "Recovered · down 4m".into(),
        }
    }

    #[test]
    fn notify_enabled_matches_spec() {
        let policy = NotifyPolicy::default();
        assert!(policy.notify_enabled());
        assert!(!policy.should_queue());

        let quiet = NotifyPolicy {
            in_quiet_hours: true,
            ..NotifyPolicy::default()
        };
        assert!(!quiet.notify_enabled());
        assert!(quiet.should_queue());

        let always = NotifyPolicy {
            in_quiet_hours: true,
            always_alert: true,
            ..NotifyPolicy::default()
        };
        assert!(always.notify_enabled());
        assert!(!always.should_queue());

        let snoozed = NotifyPolicy {
            snoozed: true,
            always_alert: true,
            ..NotifyPolicy::default()
        };
        assert!(!snoozed.notify_enabled());
        assert!(!snoozed.should_queue());

        let identity = NotifyPolicy {
            keychain_identity_changed: true,
            ..NotifyPolicy::default()
        };
        assert!(!identity.notify_enabled());
        assert!(!identity.should_queue());
    }

    #[test]
    fn two_downs_within_2s_collapse_to_digest() {
        let mut grouper = DownGrouper::new();
        assert!(grouper.push(down("a"), t(0)).is_empty());
        assert!(grouper.push(down("b"), t(1_500)).is_empty());
        let flushed = grouper.poll(t(2_000));
        assert_eq!(flushed.len(), 1);
        match &flushed[0] {
            Notification::Digest {
                service_ids,
                title,
                body,
            } => {
                assert_eq!(service_ids, &["a", "b"]);
                assert_eq!(title, "2 services down");
                assert_eq!(body, "a, b");
            }
            other => panic!("expected digest, got {other:?}"),
        }
    }

    #[test]
    fn single_down_emits_after_2s() {
        let mut grouper = DownGrouper::new();
        assert!(grouper.push(down("a"), t(0)).is_empty());
        assert!(grouper.poll(t(1_999)).is_empty());
        let flushed = grouper.poll(t(2_000));
        assert_eq!(flushed, vec![down("a")]);
    }

    #[test]
    fn downs_more_than_2s_apart_stay_individual() {
        let mut grouper = DownGrouper::new();
        assert!(grouper.push(down("a"), t(0)).is_empty());
        let first = grouper.push(down("b"), t(2_001));
        assert_eq!(first, vec![down("a")]);
        let second = grouper.poll(t(4_001));
        assert_eq!(second, vec![down("b")]);
    }

    #[test]
    fn recovery_is_never_grouped() {
        let mut grouper = DownGrouper::new();
        assert!(grouper.push(down("a"), t(0)).is_empty());
        let out = grouper.push(recovered("b"), t(100));
        assert_eq!(out, vec![recovered("b")]);
        assert_eq!(grouper.poll(t(2_000)), vec![down("a")]);
    }

    #[test]
    fn recovery_cancels_pending_down_for_same_service() {
        let mut grouper = DownGrouper::new();
        assert!(grouper.push(down("a"), t(0)).is_empty());
        let out = grouper.push(recovered("a"), t(100));
        assert_eq!(out, vec![recovered("a")]);
        assert!(grouper.poll(t(2_000)).is_empty());
    }

    #[test]
    fn recording_notifier_is_headless() {
        let mut rec = RecordingNotifier::default();
        rec.notify(down("a"));
        assert_eq!(rec.events.len(), 1);
        // Trait has no OS side effects; this type only records.
        let evidence = CheckEvidence {
            at: t(0),
            outcome: OutcomeClass::Hard,
            http_status: Some(502),
            latency_ms: Some(1400),
            redirects: None,
            headers_stripped_on_redirect: None,
            assertion_results: Vec::new(),
            assertion_skipped: None,
            error_kind: Some(ErrorKind::UnexpectedStatus),
            error: Some("HTTP 502".into()),
            body_preview: Some("secret-token".into()),
        };
        let n = Notification::down("id", "Payments API", &evidence, 10_000);
        match n {
            Notification::Down { title, body, .. } => {
                assert_eq!(title, "Payments API");
                assert_eq!(body, "HTTP 502 · 1.4s");
                assert!(!body.contains("secret-token"));
            }
            other => panic!("{other:?}"),
        }
    }
}
