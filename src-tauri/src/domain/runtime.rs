use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::ServiceStatus;

/// Persisted in SQLite `runtime_state`, never in services.json.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MachineStatus {
    Pending,
    Healthy,
    Degraded,
    Down,
}

impl MachineStatus {
    pub fn as_service_status(self) -> Option<ServiceStatus> {
        match self {
            Self::Pending => None,
            Self::Healthy => Some(ServiceStatus::Healthy),
            Self::Degraded => Some(ServiceStatus::Degraded),
            Self::Down => Some(ServiceStatus::Down),
        }
    }

    pub fn is_down(self) -> bool {
        matches!(self, Self::Down)
    }
}

/// Lives in SQLite, not in services.json.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeState {
    pub consecutive_hard_fails: u32,
    pub status: MachineStatus,
    pub down_since: Option<DateTime<Utc>>,
    pub degraded_since: Option<DateTime<Utc>>,
    pub down_clock_adjust_ms: u64,
    pub last_check_at: Option<DateTime<Utc>>,
    pub snooze_until: Option<DateTime<Utc>>,
    pub paused_at: Option<DateTime<Utc>>,
    pub slept_at: Option<DateTime<Utc>>,
}

impl RuntimeState {
    pub fn pending() -> Self {
        Self {
            consecutive_hard_fails: 0,
            status: MachineStatus::Pending,
            down_since: None,
            degraded_since: None,
            down_clock_adjust_ms: 0,
            last_check_at: None,
            snooze_until: None,
            paused_at: None,
            slept_at: None,
        }
    }

    pub fn is_snoozed(&self, now: DateTime<Utc>) -> bool {
        self.snooze_until.is_some_and(|until| now < until)
    }

    /// `now - down_since - down_clock_adjust_ms`. None when not in Down.
    pub fn displayed_down_ms(&self, now: DateTime<Utc>) -> Option<u64> {
        let down_since = self.down_since?;
        if !self.status.is_down() {
            return None;
        }
        let elapsed = now.signed_duration_since(down_since).num_milliseconds();
        let adjust = i64::try_from(self.down_clock_adjust_ms).unwrap_or(i64::MAX);
        Some(elapsed.saturating_sub(adjust).max(0) as u64)
    }
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self::pending()
    }
}
