pub mod autostart;
pub mod detail;
pub mod settings;
pub mod tray;
pub mod wake;

pub use wake::{listen, PowerEvent, WakeGuard};
pub mod detail;
pub mod autostart;
pub mod tray;
