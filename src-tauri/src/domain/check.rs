use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{AssertionResult, ErrorKind};

/// Evaluator output. Not flap-damped machine state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutcomeClass {
    Ok,
    Soft,
    Hard,
}

/// Post-machine state after on_result. Never produced by evaluate().
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceStatus {
    Healthy,
    Degraded,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiState {
    Healthy,
    Degraded,
    Down,
    Paused,
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssertionSkipped {
    Head,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SparklinePoint {
    Healthy,
    Degraded,
    Down,
    Gap,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckEvidence {
    pub at: DateTime<Utc>,
    pub outcome: OutcomeClass,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirects: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers_stripped_on_redirect: Option<bool>,
    pub assertion_results: Vec<AssertionResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assertion_skipped: Option<AssertionSkipped>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<ErrorKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_preview: Option<String>,
}

/// Live check after on_result. test_draft returns CheckEvidence, not this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    #[serde(flatten)]
    pub evidence: CheckEvidence,
    pub state: ServiceStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactSample {
    pub at: DateTime<Utc>,
    pub state: ServiceStatus,
    pub outcome: OutcomeClass,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<ErrorKind>,
}
