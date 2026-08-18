pub mod client;
<<<<<<< HEAD

pub use client::{HttpClient, BODY_PREVIEW_BYTES, MAX_BODY_BYTES, MAX_REDIRECTS, USER_AGENT};
pub mod state_machine;

=======
pub mod scheduler;
pub mod state_machine;

pub use client::{HttpClient, BODY_PREVIEW_BYTES, MAX_BODY_BYTES, MAX_REDIRECTS, USER_AGENT};
pub use scheduler::{
    should_restart, start_stagger, with_jitter, PulseEvents, Scheduler, SchedulerHandle,
    CONCURRENCY,
};
>>>>>>> 6bae09b (Scheduler, stagger, pause + logging + watchdog)
pub use state_machine::{fail_threshold, on_result, ProbeEvent, Transition};
