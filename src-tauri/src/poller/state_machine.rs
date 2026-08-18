use chrono::{DateTime, Utc};

use crate::domain::{ErrorKind, MachineStatus, RuntimeState, DEFAULT_FAIL_THRESHOLD};
use crate::eval::Outcome;
use crate::notify::{Emit, NotifyPolicy, QueueOp};

/// `canceled` / `offline` never come from `evaluate()`; they are sentinels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeEvent {
    Canceled,
    Offline,
    Applied(Outcome),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    /// False for canceled / paused / offline: do not write last_result or history.
    pub applied: bool,
    pub emit: Option<Emit>,
    pub queue: QueueOp,
}

impl Transition {
    fn silent(applied: bool) -> Self {
        Self {
            applied,
            emit: None,
            queue: QueueOp::None,
        }
    }
}

/// `service.failThreshold ?? settings.failThreshold`, default 3.
pub fn fail_threshold(service: Option<u32>, settings: u32) -> u32 {
    let n = service.unwrap_or(settings);
    if n == 0 {
        DEFAULT_FAIL_THRESHOLD
    } else {
        n
    }
}

pub fn on_result(
    runtime: &mut RuntimeState,
    event: ProbeEvent,
    now: DateTime<Utc>,
    threshold: u32,
    paused: bool,
    offline: bool,
    policy: &NotifyPolicy,
) -> Transition {
    if matches!(event, ProbeEvent::Canceled) || is_canceled_outcome(&event) {
        return Transition::silent(false);
    }
    if matches!(event, ProbeEvent::Offline) || is_offline_outcome(&event) || paused || offline {
        return Transition::silent(false);
    }

    let ProbeEvent::Applied(outcome) = event else {
        return Transition::silent(false);
    };

    let threshold = if threshold == 0 {
        DEFAULT_FAIL_THRESHOLD
    } else {
        threshold
    };

    match outcome {
        Outcome::Success { .. } => apply_success(runtime, now, policy),
        Outcome::SoftFail { .. } => apply_soft(runtime, now, policy),
        Outcome::HardFail { .. } => apply_hard(runtime, now, threshold, policy),
    }
}

fn is_canceled_outcome(event: &ProbeEvent) -> bool {
    matches!(
        event,
        ProbeEvent::Applied(Outcome::HardFail {
            kind: ErrorKind::Canceled,
            ..
        })
    )
}

fn is_offline_outcome(event: &ProbeEvent) -> bool {
    matches!(
        event,
        ProbeEvent::Applied(Outcome::HardFail {
            kind: ErrorKind::Offline,
            ..
        })
    )
}

fn apply_success(
    runtime: &mut RuntimeState,
    now: DateTime<Utc>,
    policy: &NotifyPolicy,
) -> Transition {
    let notify = take_recovery(runtime, now);
    runtime.consecutive_hard_fails = 0;
    runtime.status = MachineStatus::Healthy;
    runtime.degraded_since = None;
    runtime.last_check_at = Some(now);
    finish(notify, policy)
}

fn apply_soft(runtime: &mut RuntimeState, now: DateTime<Utc>, policy: &NotifyPolicy) -> Transition {
    // A slow 2xx is a successful reach — reset the flap counter.
    let notify = take_recovery(runtime, now);
    runtime.consecutive_hard_fails = 0;
    enter_degraded(runtime, now);
    runtime.last_check_at = Some(now);
    finish(notify, policy)
}

fn apply_hard(
    runtime: &mut RuntimeState,
    now: DateTime<Utc>,
    threshold: u32,
    policy: &NotifyPolicy,
) -> Transition {
    runtime.consecutive_hard_fails = runtime.consecutive_hard_fails.saturating_add(1);
    runtime.last_check_at = Some(now);
    let notify = if runtime.consecutive_hard_fails >= threshold {
        let entered = runtime.status != MachineStatus::Down;
        if entered {
            runtime.status = MachineStatus::Down;
            runtime.down_since = Some(now);
            runtime.degraded_since = None;
        }
        entered.then_some(StateNotify::Down)
    } else {
        enter_degraded(runtime, now);
        None
    };
    finish(notify, policy)
}

fn enter_degraded(runtime: &mut RuntimeState, now: DateTime<Utc>) {
    if runtime.status != MachineStatus::Degraded {
        runtime.degraded_since = Some(now);
    }
    runtime.status = MachineStatus::Degraded;
}

enum StateNotify {
    Down,
    Recovered { duration_ms: u64 },
}

fn take_recovery(runtime: &mut RuntimeState, now: DateTime<Utc>) -> Option<StateNotify> {
    let notify = if runtime.status == MachineStatus::Down {
        Some(StateNotify::Recovered {
            duration_ms: runtime.displayed_down_ms(now).unwrap_or(0),
        })
    } else {
        None
    };
    runtime.down_since = None;
    runtime.down_clock_adjust_ms = 0;
    notify
}

fn finish(notify: Option<StateNotify>, policy: &NotifyPolicy) -> Transition {
    match notify {
        None => Transition::silent(true),
        Some(StateNotify::Down) => decide(Emit::Down, policy),
        Some(StateNotify::Recovered { duration_ms }) => {
            decide(Emit::Recovered { duration_ms }, policy)
        }
    }
}

fn decide(emit: Emit, policy: &NotifyPolicy) -> Transition {
    if policy.notify_enabled() {
        Transition {
            applied: true,
            emit: Some(emit),
            queue: QueueOp::None,
        }
    } else if policy.should_queue() {
        let queue = match emit {
            Emit::Down => QueueOp::Enqueue,
            Emit::Recovered { .. } => QueueOp::Dequeue,
        };
        Transition {
            applied: true,
            emit: None,
            queue,
        }
    } else {
        Transition::silent(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ErrorKind;
    use crate::notify::{QueuedDown, QuietQueue};

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-18T14:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn later(secs: i64) -> DateTime<Utc> {
        t0() + chrono::Duration::seconds(secs)
    }

    fn success() -> ProbeEvent {
        ProbeEvent::Applied(Outcome::Success {
            http_status: 200,
            latency_ms: 50,
            redirects: 0,
        })
    }

    fn soft() -> ProbeEvent {
        ProbeEvent::Applied(Outcome::SoftFail {
            kind: ErrorKind::Slow,
            http_status: 200,
            latency_ms: 900,
        })
    }

    fn hard() -> ProbeEvent {
        ProbeEvent::Applied(Outcome::HardFail {
            kind: ErrorKind::Timeout,
            http_status: None,
            latency_ms: Some(10_000),
        })
    }

    fn apply(
        runtime: &mut RuntimeState,
        event: ProbeEvent,
        now: DateTime<Utc>,
        threshold: u32,
        policy: &NotifyPolicy,
    ) -> Transition {
        on_result(runtime, event, now, threshold, false, false, policy)
    }

    #[test]
    fn fail_threshold_defaults_to_3() {
        assert_eq!(fail_threshold(None, 3), 3);
        assert_eq!(fail_threshold(None, 0), 3);
        assert_eq!(fail_threshold(Some(5), 3), 5);
        assert_eq!(DEFAULT_FAIL_THRESHOLD, 3);
    }

    #[test]
    fn pending_soft_fail_becomes_degraded_slow_no_notify() {
        let mut runtime = RuntimeState::pending();
        let transition = apply(&mut runtime, soft(), t0(), 3, &NotifyPolicy::default());
        assert!(transition.applied);
        assert_eq!(runtime.status, MachineStatus::Degraded);
        assert_eq!(runtime.consecutive_hard_fails, 0);
        assert_eq!(runtime.down_since, None);
        assert_eq!(runtime.degraded_since, Some(t0()));
        assert_eq!(runtime.last_check_at, Some(t0()));
        assert_eq!(transition.emit, None);
        assert_eq!(transition.queue, QueueOp::None);
    }

    #[test]
    fn degraded_since_stays_until_leave() {
        let mut runtime = RuntimeState::pending();
        let policy = NotifyPolicy::default();
        apply(&mut runtime, soft(), t0(), 3, &policy);
        assert_eq!(runtime.degraded_since, Some(t0()));
        apply(&mut runtime, soft(), later(180), 3, &policy);
        assert_eq!(runtime.degraded_since, Some(t0()));
        apply(&mut runtime, hard(), later(240), 3, &policy);
        assert_eq!(runtime.status, MachineStatus::Degraded);
        assert_eq!(runtime.degraded_since, Some(t0()));
        apply(&mut runtime, success(), later(300), 3, &policy);
        assert_eq!(runtime.status, MachineStatus::Healthy);
        assert_eq!(runtime.degraded_since, None);
    }

    #[test]
    fn canceled_is_noop() {
        let mut runtime = RuntimeState::pending();
        runtime.consecutive_hard_fails = 2;
        runtime.status = MachineStatus::Degraded;
        runtime.last_check_at = Some(t0());
        let before = runtime.clone();

        let a = on_result(
            &mut runtime,
            ProbeEvent::Canceled,
            later(60),
            3,
            false,
            false,
            &NotifyPolicy::default(),
        );
        assert!(!a.applied);
        assert_eq!(runtime, before);

        let b = on_result(
            &mut runtime,
            ProbeEvent::Applied(Outcome::HardFail {
                kind: ErrorKind::Canceled,
                http_status: None,
                latency_ms: None,
            }),
            later(60),
            3,
            false,
            false,
            &NotifyPolicy::default(),
        );
        assert!(!b.applied);
        assert_eq!(runtime, before);
    }

    #[test]
    fn paused_or_offline_is_noop() {
        let mut runtime = RuntimeState::pending();
        let before = runtime.clone();
        assert!(
            !on_result(
                &mut runtime,
                success(),
                t0(),
                3,
                true,
                false,
                &NotifyPolicy::default()
            )
            .applied
        );
        assert_eq!(runtime, before);
        assert!(
            !on_result(
                &mut runtime,
                hard(),
                t0(),
                3,
                false,
                true,
                &NotifyPolicy::default()
            )
            .applied
        );
        assert_eq!(runtime, before);
        assert!(
            !on_result(
                &mut runtime,
                ProbeEvent::Offline,
                t0(),
                3,
                false,
                false,
                &NotifyPolicy::default()
            )
            .applied
        );
        assert_eq!(runtime, before);
    }

    #[test]
    fn transition_table() {
        struct Row {
            name: &'static str,
            start: MachineStatus,
            fails: u32,
            event: fn() -> ProbeEvent,
            threshold: u32,
            expect_status: MachineStatus,
            expect_fails: u32,
            expect_emit: Option<Emit>,
        }

        let rows = [
            Row {
                name: "pending + success → healthy",
                start: MachineStatus::Pending,
                fails: 0,
                event: success,
                threshold: 3,
                expect_status: MachineStatus::Healthy,
                expect_fails: 0,
                expect_emit: None,
            },
            Row {
                name: "pending + soft → degraded, no notify",
                start: MachineStatus::Pending,
                fails: 0,
                event: soft,
                threshold: 3,
                expect_status: MachineStatus::Degraded,
                expect_fails: 0,
                expect_emit: None,
            },
            Row {
                name: "pending + hard < N → degraded",
                start: MachineStatus::Pending,
                fails: 0,
                event: hard,
                threshold: 3,
                expect_status: MachineStatus::Degraded,
                expect_fails: 1,
                expect_emit: None,
            },
            Row {
                name: "pending + hard with N=1 → down",
                start: MachineStatus::Pending,
                fails: 0,
                event: hard,
                threshold: 1,
                expect_status: MachineStatus::Down,
                expect_fails: 1,
                expect_emit: Some(Emit::Down),
            },
            Row {
                name: "healthy + soft → degraded",
                start: MachineStatus::Healthy,
                fails: 0,
                event: soft,
                threshold: 3,
                expect_status: MachineStatus::Degraded,
                expect_fails: 0,
                expect_emit: None,
            },
            Row {
                name: "healthy + hard → degraded",
                start: MachineStatus::Healthy,
                fails: 0,
                event: hard,
                threshold: 3,
                expect_status: MachineStatus::Degraded,
                expect_fails: 1,
                expect_emit: None,
            },
            Row {
                name: "degraded Nth hard → down + notify",
                start: MachineStatus::Degraded,
                fails: 2,
                event: hard,
                threshold: 3,
                expect_status: MachineStatus::Down,
                expect_fails: 3,
                expect_emit: Some(Emit::Down),
            },
            Row {
                name: "degraded + more hard below N stays degraded",
                start: MachineStatus::Degraded,
                fails: 1,
                event: hard,
                threshold: 3,
                expect_status: MachineStatus::Degraded,
                expect_fails: 2,
                expect_emit: None,
            },
            Row {
                name: "soft fail resets consecutive_hard_fails",
                start: MachineStatus::Degraded,
                fails: 2,
                event: soft,
                threshold: 3,
                expect_status: MachineStatus::Degraded,
                expect_fails: 0,
                expect_emit: None,
            },
            Row {
                name: "down + hard stays down, no re-notify",
                start: MachineStatus::Down,
                fails: 3,
                event: hard,
                threshold: 3,
                expect_status: MachineStatus::Down,
                expect_fails: 4,
                expect_emit: None,
            },
            Row {
                name: "down + success → healthy + recovered",
                start: MachineStatus::Down,
                fails: 3,
                event: success,
                threshold: 3,
                expect_status: MachineStatus::Healthy,
                expect_fails: 0,
                expect_emit: Some(Emit::Recovered { duration_ms: 0 }),
            },
            Row {
                name: "down + soft → degraded + recovered",
                start: MachineStatus::Down,
                fails: 3,
                event: soft,
                threshold: 3,
                expect_status: MachineStatus::Degraded,
                expect_fails: 0,
                expect_emit: Some(Emit::Recovered { duration_ms: 0 }),
            },
            Row {
                name: "degraded + success → healthy, no notify",
                start: MachineStatus::Degraded,
                fails: 1,
                event: success,
                threshold: 3,
                expect_status: MachineStatus::Healthy,
                expect_fails: 0,
                expect_emit: None,
            },
        ];

        for row in rows {
            let mut runtime = RuntimeState::pending();
            runtime.status = row.start;
            runtime.consecutive_hard_fails = row.fails;
            if row.start == MachineStatus::Down {
                runtime.down_since = Some(t0());
            }
            let transition = apply(
                &mut runtime,
                (row.event)(),
                t0(),
                row.threshold,
                &NotifyPolicy::default(),
            );
            assert_eq!(runtime.status, row.expect_status, "{}", row.name);
            assert_eq!(
                runtime.consecutive_hard_fails, row.expect_fails,
                "{}",
                row.name
            );
            assert_eq!(transition.emit, row.expect_emit, "{}", row.name);
            assert!(transition.applied, "{}", row.name);
        }
    }

    #[test]
    fn recovery_duration_uses_down_clock_adjust() {
        let mut runtime = RuntimeState::pending();
        runtime.status = MachineStatus::Down;
        runtime.consecutive_hard_fails = 3;
        runtime.down_since = Some(t0());
        runtime.down_clock_adjust_ms = 2 * 60 * 1_000;
        let now = later(6 * 60);
        let transition = apply(&mut runtime, success(), now, 3, &NotifyPolicy::default());
        assert_eq!(runtime.status, MachineStatus::Healthy);
        assert_eq!(runtime.down_since, None);
        assert_eq!(runtime.degraded_since, None);
        assert_eq!(runtime.down_clock_adjust_ms, 0);
        match transition.emit {
            Some(Emit::Recovered { duration_ms }) => {
                assert_eq!(duration_ms, 4 * 60 * 1_000);
            }
            other => panic!("expected recovered, got {other:?}"),
        }
    }

    #[test]
    fn snooze_suppresses_down_and_recovery() {
        let policy = NotifyPolicy {
            snoozed: true,
            always_alert: true,
            ..NotifyPolicy::default()
        };
        let mut runtime = RuntimeState::pending();
        runtime.status = MachineStatus::Degraded;
        runtime.consecutive_hard_fails = 2;
        let down = apply(&mut runtime, hard(), t0(), 3, &policy);
        assert_eq!(runtime.status, MachineStatus::Down);
        assert_eq!(down.emit, None);
        assert_eq!(down.queue, QueueOp::None);

        runtime.down_since = Some(t0());
        let rec = apply(&mut runtime, success(), later(60), 3, &policy);
        assert_eq!(runtime.status, MachineStatus::Healthy);
        assert_eq!(rec.emit, None);
        assert_eq!(rec.queue, QueueOp::None);
    }

    #[test]
    fn keychain_identity_changed_suppresses_notify() {
        let policy = NotifyPolicy {
            keychain_identity_changed: true,
            ..NotifyPolicy::default()
        };
        let mut runtime = RuntimeState::pending();
        runtime.status = MachineStatus::Degraded;
        runtime.consecutive_hard_fails = 2;
        let down = apply(&mut runtime, hard(), t0(), 3, &policy);
        assert_eq!(runtime.status, MachineStatus::Down);
        assert_eq!(down.emit, None);
        assert_eq!(down.queue, QueueOp::None);
    }

    #[test]
    fn quiet_hours_enqueue_down_and_cancel_out_on_recover() {
        let policy = NotifyPolicy {
            in_quiet_hours: true,
            ..NotifyPolicy::default()
        };
        let mut runtime = RuntimeState::pending();
        runtime.status = MachineStatus::Degraded;
        runtime.consecutive_hard_fails = 2;
        let down = apply(&mut runtime, hard(), t0(), 3, &policy);
        assert_eq!(runtime.status, MachineStatus::Down);
        assert_eq!(down.emit, None);
        assert_eq!(down.queue, QueueOp::Enqueue);

        let mut queue = QuietQueue::new();
        queue.apply(
            down.queue,
            QueuedDown {
                service_id: "pay".into(),
                name: "Payments".into(),
                title: "Payments".into(),
                body: "HTTP 502 · 1.4s".into(),
            },
        );
        assert!(queue.contains("pay"));

        runtime.down_since = Some(t0());
        let rec = apply(&mut runtime, success(), later(30), 3, &policy);
        assert_eq!(runtime.status, MachineStatus::Healthy);
        assert_eq!(rec.emit, None);
        assert_eq!(rec.queue, QueueOp::Dequeue);
        queue.apply(
            rec.queue,
            QueuedDown {
                service_id: "pay".into(),
                name: "Payments".into(),
                title: "Payments".into(),
                body: String::new(),
            },
        );
        assert!(queue.is_empty());
    }

    #[test]
    fn always_alert_bypasses_quiet_hours() {
        let policy = NotifyPolicy {
            in_quiet_hours: true,
            always_alert: true,
            ..NotifyPolicy::default()
        };
        let mut runtime = RuntimeState::pending();
        runtime.status = MachineStatus::Degraded;
        runtime.consecutive_hard_fails = 2;
        let down = apply(&mut runtime, hard(), t0(), 3, &policy);
        assert_eq!(down.emit, Some(Emit::Down));
        assert_eq!(down.queue, QueueOp::None);
    }

    #[test]
    fn three_consecutive_hard_fails_from_pending() {
        let mut runtime = RuntimeState::pending();
        let policy = NotifyPolicy::default();
        let a = apply(&mut runtime, hard(), t0(), 3, &policy);
        assert_eq!(runtime.status, MachineStatus::Degraded);
        assert_eq!(runtime.degraded_since, Some(t0()));
        assert_eq!(a.emit, None);
        let b = apply(&mut runtime, hard(), later(60), 3, &policy);
        assert_eq!(runtime.status, MachineStatus::Degraded);
        assert_eq!(runtime.degraded_since, Some(t0()));
        assert_eq!(b.emit, None);
        let c = apply(&mut runtime, hard(), later(120), 3, &policy);
        assert_eq!(runtime.status, MachineStatus::Down);
        assert_eq!(runtime.down_since, Some(later(120)));
        assert_eq!(runtime.degraded_since, None);
        assert_eq!(c.emit, Some(Emit::Down));
    }

    #[test]
    fn never_returns_to_pending() {
        let mut runtime = RuntimeState::pending();
        apply(&mut runtime, success(), t0(), 3, &NotifyPolicy::default());
        assert_eq!(runtime.status, MachineStatus::Healthy);
        apply(&mut runtime, hard(), later(60), 3, &NotifyPolicy::default());
        assert_ne!(runtime.status, MachineStatus::Pending);
        apply(
            &mut runtime,
            soft(),
            later(120),
            3,
            &NotifyPolicy::default(),
        );
        assert_ne!(runtime.status, MachineStatus::Pending);
    }
}
