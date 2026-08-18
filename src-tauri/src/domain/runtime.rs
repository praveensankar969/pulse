use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Persisted in SQLite `runtime_state`, never in services.json.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MachineStatus {
    Pending,
    Healthy,
    Degraded,
    Down,
}

/// Lives in SQLite, not in services.json.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeState {
    pub consecutive_hard_fails: u32,
    pub status: MachineStatus,
    pub down_since: Option<DateTime<Utc>>,
    pub down_clock_adjust_ms: u64,
    pub last_check_at: Option<DateTime<Utc>>,
    pub snooze_until: Option<DateTime<Utc>>,
    pub paused_at: Option<DateTime<Utc>>,
    pub slept_at: Option<DateTime<Utc>>,
}
