//! What the driver needs from the platform it runs on: spawning a task, sleeping, and the two clocks (a monotonic
//! one for the state machine, Unix time for the turns' timestamps). Each platform's file says where it runs.

mod native;
mod web;

#[cfg(native)]
pub(crate) use native::{monotonic_ms, sleep, spawn, unix_ms};
#[cfg(web)]
pub(crate) use web::{monotonic_ms, sleep, spawn, unix_ms};
