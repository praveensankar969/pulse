mod classify;
mod compare;
mod path;

pub use classify::{evaluate, evaluate_at, outcome_of};
pub use compare::compare;
pub use path::{parse_path, resolve_path, PathError, Segment};

use crate::domain::ErrorKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawResponse {
    pub status: u16,
    pub latency_ms: u32,
    pub redirects: u8,
    pub headers_stripped_on_redirect: bool,
    pub body: Vec<u8>,
    pub body_preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError {
    pub kind: ErrorKind,
    pub latency_ms: Option<u32>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Success {
        http_status: u16,
        latency_ms: u32,
        redirects: u8,
    },
    SoftFail {
        kind: ErrorKind,
        http_status: u16,
        latency_ms: u32,
    },
    HardFail {
        kind: ErrorKind,
        http_status: Option<u16>,
        latency_ms: Option<u32>,
    },
}
