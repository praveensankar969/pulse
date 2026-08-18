use std::collections::HashSet;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::domain::ErrorKind;

pub use crate::domain::MIXED_REACHABILITY_HELP;

pub const OFFLINE_WINDOW: Duration = Duration::from_secs(90);
pub const WAKE_GRACE: Duration = Duration::from_secs(15);
pub const RESUME_SETTLE: Duration = Duration::from_secs(2);

/// Unreachable | Dns | Timeout (connect/request; no separate ConnectTimeout kind).
pub fn is_offline_signal(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::Unreachable | ErrorKind::Dns | ErrorKind::Timeout
    )
}

/// NIC-not-ready class used for the 15s post-wake grace.
pub fn is_transport_error(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::Timeout
            | ErrorKind::Dns
            | ErrorKind::Unreachable
            | ErrorKind::Refused
            | ErrorKind::Reset
    )
}

/// `now - next_due > 2 * interval`. Wall clock — Instant does not tick during lid-close.
pub fn is_overdue(now: DateTime<Utc>, next_due: DateTime<Utc>, interval: Duration) -> bool {
    let Ok(limit) = chrono::Duration::from_std(interval.saturating_mul(2)) else {
        return false;
    };
    now.signed_duration_since(next_due) > limit
}

pub fn in_wake_grace(now: DateTime<Utc>, wake_at: DateTime<Utc>) -> bool {
    now.signed_duration_since(wake_at) < chrono::Duration::from_std(WAKE_GRACE).expect("15s")
}

pub fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_ascii_lowercase))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfflineTransition {
    None,
    Entered,
    Exited { entered_at: DateTime<Utc> },
}

#[derive(Debug, Clone)]
struct HostFail {
    host: String,
    at: DateTime<Utc>,
}

/// ≥2 unpaused services and ≥2 distinct hosts with transport-class fails in 90s.
/// Any success (ok or soft) exits and blocks re-entry until those fails age out.
#[derive(Debug, Default)]
pub struct OfflineDetector {
    offline: bool,
    entered_at: Option<DateTime<Utc>>,
    last_success: Option<DateTime<Utc>>,
    failures: Vec<HostFail>,
}

impl OfflineDetector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_offline(&self) -> bool {
        self.offline
    }

    pub fn entered_at(&self) -> Option<DateTime<Utc>> {
        self.entered_at
    }

    pub fn observe(
        &mut self,
        host: Option<&str>,
        kind: Option<ErrorKind>,
        reached: bool,
        unpaused_services: usize,
        now: DateTime<Utc>,
    ) -> OfflineTransition {
        if reached {
            self.last_success = Some(now);
            self.prune(now);
            return self.exit(now);
        }

        if let (Some(host), Some(kind)) = (host, kind) {
            if is_offline_signal(kind) {
                self.failures.push(HostFail {
                    host: host.to_string(),
                    at: now,
                });
            }
        }
        self.prune(now);

        if !self.offline
            && unpaused_services >= 2
            && self.distinct_fail_hosts() >= 2
            && !self.recent_success(now)
        {
            self.offline = true;
            self.entered_at = Some(now);
            return OfflineTransition::Entered;
        }
        OfflineTransition::None
    }

    fn recent_success(&self, now: DateTime<Utc>) -> bool {
        let window = chrono::Duration::from_std(OFFLINE_WINDOW).expect("90s");
        self.last_success
            .is_some_and(|success| now.signed_duration_since(success) <= window)
    }

    fn exit(&mut self, _now: DateTime<Utc>) -> OfflineTransition {
        if !self.offline {
            return OfflineTransition::None;
        }
        self.offline = false;
        let entered_at = self.entered_at.take().unwrap_or(_now);
        OfflineTransition::Exited { entered_at }
    }

    fn prune(&mut self, now: DateTime<Utc>) {
        let window = chrono::Duration::from_std(OFFLINE_WINDOW).expect("90s");
        self.failures.retain(|fail| {
            now.signed_duration_since(fail.at) <= window
                && self.last_success.is_none_or(|success| fail.at > success)
        });
    }

    fn distinct_fail_hosts(&self) -> usize {
        self.failures
            .iter()
            .map(|fail| fail.host.as_str())
            .collect::<HashSet<_>>()
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-18T14:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn later(secs: i64) -> DateTime<Utc> {
        t0() + chrono::Duration::seconds(secs)
    }

    #[test]
    fn mixed_reachability_help_is_verbatim() {
        assert_eq!(
            MIXED_REACHABILITY_HELP,
            "If any check succeeds, Pulse assumes the network is up. A homelab box that still answers will keep Pulse online even if the public internet is gone."
        );
    }

    #[test]
    fn overdue_is_strictly_more_than_two_intervals_past_due() {
        let interval = Duration::from_secs(60);
        let due = t0() + chrono::Duration::seconds(60);
        assert!(!is_overdue(due, due, interval));
        assert!(!is_overdue(
            due + chrono::Duration::seconds(120),
            due,
            interval
        ));
        assert!(is_overdue(
            due + chrono::Duration::seconds(121),
            due,
            interval
        ));
    }

    #[test]
    fn wake_grace_is_first_15s() {
        assert!(in_wake_grace(t0(), t0()));
        assert!(in_wake_grace(later(14), t0()));
        assert!(!in_wake_grace(later(15), t0()));
    }

    #[test]
    fn host_is_lowercase_without_port() {
        assert_eq!(
            host_of("https://API.Example.com:8443/health").as_deref(),
            Some("api.example.com")
        );
        assert_eq!(host_of("not a url"), None);
    }

    #[test]
    fn single_service_never_goes_offline() {
        let mut det = OfflineDetector::new();
        let change = det.observe(Some("a.example"), Some(ErrorKind::Dns), false, 1, t0());
        assert_eq!(change, OfflineTransition::None);
        assert!(!det.is_offline());
    }

    #[test]
    fn two_hosts_same_service_count_is_not_enough_without_two_unpaused() {
        let mut det = OfflineDetector::new();
        det.observe(Some("a.example"), Some(ErrorKind::Dns), false, 1, t0());
        let change = det.observe(
            Some("b.example"),
            Some(ErrorKind::Unreachable),
            false,
            1,
            later(1),
        );
        assert_eq!(change, OfflineTransition::None);
        assert!(!det.is_offline());
    }

    #[test]
    fn two_services_same_host_is_not_offline() {
        let mut det = OfflineDetector::new();
        det.observe(Some("nas.local"), Some(ErrorKind::Timeout), false, 2, t0());
        let change = det.observe(
            Some("nas.local"),
            Some(ErrorKind::Timeout),
            false,
            2,
            later(1),
        );
        assert_eq!(change, OfflineTransition::None);
        assert!(!det.is_offline());
    }

    #[test]
    fn two_hosts_enter_offline() {
        let mut det = OfflineDetector::new();
        assert_eq!(
            det.observe(Some("a.example"), Some(ErrorKind::Dns), false, 2, t0()),
            OfflineTransition::None
        );
        assert_eq!(
            det.observe(
                Some("b.example"),
                Some(ErrorKind::Unreachable),
                false,
                2,
                later(1)
            ),
            OfflineTransition::Entered
        );
        assert!(det.is_offline());
    }

    #[test]
    fn http_500_is_not_an_offline_signal() {
        let mut det = OfflineDetector::new();
        det.observe(
            Some("a.example"),
            Some(ErrorKind::UnexpectedStatus),
            false,
            2,
            t0(),
        );
        let change = det.observe(
            Some("b.example"),
            Some(ErrorKind::UnexpectedStatus),
            false,
            2,
            later(1),
        );
        assert_eq!(change, OfflineTransition::None);
    }

    #[test]
    fn success_exits_and_blocks_reentry_while_recent() {
        let mut det = OfflineDetector::new();
        det.observe(Some("a.example"), Some(ErrorKind::Dns), false, 2, t0());
        det.observe(
            Some("b.example"),
            Some(ErrorKind::Timeout),
            false,
            2,
            later(1),
        );
        assert!(det.is_offline());
        let change = det.observe(Some("nas.local"), None, true, 3, later(2));
        assert_eq!(
            change,
            OfflineTransition::Exited {
                entered_at: later(1)
            }
        );
        assert!(!det.is_offline());

        // Public hosts still fail; NAS success keeps us online (mixed reachability).
        assert_eq!(
            det.observe(Some("a.example"), Some(ErrorKind::Dns), false, 3, later(3)),
            OfflineTransition::None
        );
        assert_eq!(
            det.observe(
                Some("b.example"),
                Some(ErrorKind::Timeout),
                false,
                3,
                later(4)
            ),
            OfflineTransition::None
        );
        assert!(!det.is_offline());
    }

    #[test]
    fn soft_reach_counts_as_success() {
        let mut det = OfflineDetector::new();
        det.observe(Some("a.example"), Some(ErrorKind::Dns), false, 2, t0());
        det.observe(
            Some("b.example"),
            Some(ErrorKind::Timeout),
            false,
            2,
            later(1),
        );
        assert_eq!(
            det.observe(Some("a.example"), Some(ErrorKind::Slow), true, 2, later(2)),
            OfflineTransition::Exited {
                entered_at: later(1)
            }
        );
    }

    #[test]
    fn failures_older_than_90s_do_not_count() {
        let mut det = OfflineDetector::new();
        det.observe(Some("a.example"), Some(ErrorKind::Dns), false, 2, t0());
        let change = det.observe(
            Some("b.example"),
            Some(ErrorKind::Timeout),
            false,
            2,
            later(91),
        );
        assert_eq!(change, OfflineTransition::None);
        assert!(!det.is_offline());
    }

    #[test]
    fn can_reenter_after_success_ages_out() {
        let mut det = OfflineDetector::new();
        det.observe(Some("a.example"), Some(ErrorKind::Dns), false, 2, t0());
        det.observe(
            Some("b.example"),
            Some(ErrorKind::Timeout),
            false,
            2,
            later(1),
        );
        det.observe(Some("nas.local"), None, true, 3, later(2));
        assert!(!det.is_offline());

        det.observe(Some("a.example"), Some(ErrorKind::Dns), false, 2, later(93));
        let change = det.observe(
            Some("b.example"),
            Some(ErrorKind::Timeout),
            false,
            2,
            later(94),
        );
        assert_eq!(change, OfflineTransition::Entered);
    }
}
