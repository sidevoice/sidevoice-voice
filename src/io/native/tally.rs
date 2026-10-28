//! What a microphone stream delivered in its first seconds — its callbacks, its samples and the glitches it reported —
//! said once, as a log line. Counts only: nothing of what was heard.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long a stream is counted before its tally is said.
pub(super) const FIRST_SECONDS: Duration = Duration::from_secs(2);

/// One stream's counts since it opened, written from its callback and its error callback.
pub(super) struct Tally {
    opened: Instant,
    callbacks: AtomicU64,
    samples: AtomicU64,
    glitches: AtomicU64,
    said: AtomicBool,
}

impl Tally {
    pub(super) fn new(opened: Instant) -> Self {
        Self {
            opened,
            callbacks: AtomicU64::new(0),
            samples: AtomicU64::new(0),
            glitches: AtomicU64::new(0),
            said: AtomicBool::new(false),
        }
    }

    /// A callback that brought `samples` interleaved samples.
    pub(super) fn heard(&self, samples: usize) {
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        self.samples.fetch_add(samples as u64, Ordering::Relaxed);
    }

    /// A glitch the stream reported and went on from.
    pub(super) fn glitched(&self) {
        self.glitches.fetch_add(1, Ordering::Relaxed);
    }

    /// The line to log, once `FIRST_SECONDS` have passed at `now`, and only the first time.
    pub(super) fn due(&self, now: Instant) -> Option<String> {
        if now.duration_since(self.opened) < FIRST_SECONDS
            || self.said.swap(true, Ordering::Relaxed)
        {
            return None;
        }
        Some(format!(
            "microphone stream, first {} s: {} callbacks, {} samples, {} glitches",
            FIRST_SECONDS.as_secs(),
            self.callbacks.load(Ordering::Relaxed),
            self.samples.load(Ordering::Relaxed),
            self.glitches.load(Ordering::Relaxed),
        ))
    }
}
