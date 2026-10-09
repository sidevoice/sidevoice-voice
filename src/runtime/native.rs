//! Tokio's: the engine's native futures expect a Tokio runtime, the app's own.
#![cfg(native)]

use std::future::Future;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Runs `task` on the app's Tokio runtime.
pub(crate) fn spawn(task: impl Future<Output = ()> + Send + 'static) {
    tokio::spawn(task);
}

/// Waits `ms` milliseconds.
pub(crate) async fn sleep(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

/// Milliseconds since the first call, monotonic.
pub(crate) fn monotonic_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Milliseconds since the Unix epoch.
pub(crate) fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}
