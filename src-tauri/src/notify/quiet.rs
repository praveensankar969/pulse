use chrono::{DateTime, Datelike, NaiveDateTime, NaiveTime, TimeZone};

use crate::domain::QuietHours;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueueOp {
    #[default]
    None,
    Enqueue,
    Dequeue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedDown {
    pub service_id: String,
    pub name: String,
    pub title: String,
    pub body: String,
}

/// Services that **entered Down during this quiet window** and are still Down.
///
/// Membership is the queue, not the current worst-of. Flush is PR 15.
#[derive(Debug, Default, Clone)]
pub struct QuietQueue {
    entries: Vec<QueuedDown>,
}

impl QuietQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, op: QueueOp, entry: QueuedDown) {
        match op {
            QueueOp::None => {}
            QueueOp::Enqueue => self.enter(entry),
            QueueOp::Dequeue => self.recover(&entry.service_id),
        }
    }

    pub fn enter(&mut self, entry: QueuedDown) {
        if !self.contains(&entry.service_id) {
            self.entries.push(entry);
        }
    }

    pub fn recover(&mut self, service_id: &str) {
        self.entries.retain(|entry| entry.service_id != service_id);
    }

    pub fn contains(&self, service_id: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.service_id == service_id)
    }

    pub fn members(&self) -> &[QueuedDown] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

pub fn in_quiet_hours<Tz: TimeZone>(hours: &QuietHours, now: DateTime<Tz>) -> bool {
    in_quiet_window(hours, now.naive_local())
}

/// `days` names the day the window **starts**. Overnight `[start, next+end)`
/// continues even when `D+1` is unchecked (Friday 22:00 → Saturday 08:00).
pub fn in_quiet_window(hours: &QuietHours, local: NaiveDateTime) -> bool {
    let Some(start) = parse_hhmm(&hours.start) else {
        return false;
    };
    let Some(end) = parse_hhmm(&hours.end) else {
        return false;
    };
    if hours.days.is_empty() {
        return false;
    }

    let weekday = local.weekday().num_days_from_sunday() as u8;
    let yesterday = (weekday + 6) % 7;
    let selected = |day: u8| hours.days.contains(&day);
    let time = local.time();

    if start < end {
        selected(weekday) && time >= start && time < end
    } else {
        (selected(weekday) && time >= start) || (selected(yesterday) && time < end)
    }
}

fn parse_hhmm(value: &str) -> Option<NaiveTime> {
    let bytes = value.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' {
        return None;
    }
    let hour: u32 = value[0..2].parse().ok()?;
    let minute: u32 = value[3..5].parse().ok()?;
    NaiveTime::from_hms_opt(hour, minute, 0)
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn ndt(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_opt(hour, minute, 0)
            .unwrap()
    }

    fn weekdays_2200_0800() -> QuietHours {
        QuietHours {
            start: "22:00".into(),
            end: "08:00".into(),
            days: vec![1, 2, 3, 4, 5],
        }
    }

    #[test]
    fn overnight_friday_covers_saturday_morning_even_if_saturday_unchecked() {
        let hours = weekdays_2200_0800();
        // 2026-08-21 is Friday; 22nd is Saturday (unchecked).
        assert!(!in_quiet_window(&hours, ndt(2026, 8, 21, 21, 59)));
        assert!(in_quiet_window(&hours, ndt(2026, 8, 21, 22, 0)));
        assert!(in_quiet_window(&hours, ndt(2026, 8, 21, 23, 30)));
        assert!(in_quiet_window(&hours, ndt(2026, 8, 22, 0, 0)));
        assert!(in_quiet_window(&hours, ndt(2026, 8, 22, 7, 59)));
        assert!(!in_quiet_window(&hours, ndt(2026, 8, 22, 8, 0)));
        assert!(!in_quiet_window(&hours, ndt(2026, 8, 22, 22, 0)));
        // Saturday did not start a window, so Sunday morning is open.
        assert!(!in_quiet_window(&hours, ndt(2026, 8, 23, 7, 59)));
    }

    #[test]
    fn same_day_window_does_not_spill() {
        let hours = QuietHours {
            start: "09:00".into(),
            end: "17:00".into(),
            days: vec![1, 2, 3, 4, 5],
        };
        assert!(!in_quiet_window(&hours, ndt(2026, 8, 21, 8, 59)));
        assert!(in_quiet_window(&hours, ndt(2026, 8, 21, 9, 0)));
        assert!(in_quiet_window(&hours, ndt(2026, 8, 21, 16, 59)));
        assert!(!in_quiet_window(&hours, ndt(2026, 8, 21, 17, 0)));
        assert!(!in_quiet_window(&hours, ndt(2026, 8, 22, 10, 0)));
    }

    #[test]
    fn empty_days_is_never_quiet() {
        let hours = QuietHours {
            start: "22:00".into(),
            end: "08:00".into(),
            days: vec![],
        };
        assert!(!in_quiet_window(&hours, ndt(2026, 8, 21, 23, 0)));
    }

    fn entry(id: &str) -> QueuedDown {
        QueuedDown {
            service_id: id.into(),
            name: id.into(),
            title: id.into(),
            body: "HTTP 502 · 1.4s".into(),
        }
    }

    #[test]
    fn queue_membership_is_entered_down_during_window() {
        let mut queue = QuietQueue::new();
        // Already-down before the window never enters — only apply(Enqueue) does.
        queue.apply(QueueOp::Enqueue, entry("payments"));
        queue.apply(QueueOp::Enqueue, entry("worker"));
        assert_eq!(queue.len(), 2);
        assert!(queue.contains("payments"));
        assert!(queue.contains("worker"));
        assert!(!queue.contains("nas"));
    }

    #[test]
    fn down_then_recovered_during_window_cancels_out() {
        let mut queue = QuietQueue::new();
        queue.apply(QueueOp::Enqueue, entry("payments"));
        queue.apply(QueueOp::Enqueue, entry("worker"));
        queue.apply(QueueOp::Dequeue, entry("payments"));
        assert!(!queue.contains("payments"));
        assert_eq!(
            queue
                .members()
                .iter()
                .map(|e| e.service_id.as_str())
                .collect::<Vec<_>>(),
            ["worker"]
        );
    }

    #[test]
    fn enqueue_is_idempotent() {
        let mut queue = QuietQueue::new();
        queue.enter(entry("payments"));
        queue.enter(entry("payments"));
        assert_eq!(queue.len(), 1);
    }
}
