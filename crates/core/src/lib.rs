//! Resumer core: session state, hook handling, scheduling, process discovery and console input.
//! Everything except `proc` and `console` is pure logic and unit-tested.

pub mod config;
pub mod console;
#[cfg(windows)]
pub mod engine;
pub mod hooks;
pub mod i18n;
pub mod localtime;
pub mod logger;
pub mod paths;
pub mod proc;
pub mod schedule;
pub mod settings_merge;
pub mod state;
pub mod targets;

pub use state::{SessionState, Status, Window};

/// Current wall-clock time as unix seconds.
pub fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
