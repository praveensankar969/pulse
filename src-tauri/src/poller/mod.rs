pub mod client;

pub use client::{HttpClient, BODY_PREVIEW_BYTES, MAX_BODY_BYTES, MAX_REDIRECTS, USER_AGENT};
pub mod state_machine;

pub mod offline;
pub mod scheduler;
pub mod state_machine;

pub use client::{HttpClient, BODY_PREVIEW_BYTES, MAX_BODY_BYTES, MAX_REDIRECTS, USER_AGENT};
pub use offline::{
    host_of, in_wake_grace, is_offline_signal, is_overdue, is_transport_error, offline_adjust_ms,
    OfflineDetector, OfflineTransition, MIXED_REACHABILITY_HELP, OFFLINE_WINDOW, RESUME_SETTLE,
    WAKE_GRACE,
};
pub use scheduler::{
    should_restart, start_stagger, with_jitter, PulseEvents, Scheduler, SchedulerHandle,
    CONCURRENCY,
};
pub use state_machine::{fail_threshold, on_result, ProbeEvent, Transition};
