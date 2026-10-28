//! What a microphone stream delivered in its first seconds — its callbacks, its samples and the glitches it reported —
//! handed out once. Counts only: nothing of what was heard.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long a stream is counted before its tally is handed out.
pub(super) const FIRST_SECONDS: Duration = Duration::from_secs(2);

/// One stream's counts since it opened, written from its callback and its error callback.
pub(super) struct Tally {
    opened: Instant,
    callbacks: AtomicU64,
    samples: AtomicU64,
    glitches: AtomicU64,
    handed: AtomicBool,
}

/// What a stream delivered in its first seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Counts {
    pub(super) callbacks: u64,
    pub(super) samples: u64,
    pub(super) glitches: u64,
}

impl Tally {
    pub(super) fn new(opened: Instant) -> Self {
        Self {
            opened,
            callbacks: AtomicU64::new(0),
            samples: AtomicU64::new(0),
            glitches: AtomicU64::new(0),
            handed: AtomicBool::new(false),
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

    /// The counts so far.
    pub(super) fn counts(&self) -> Counts {
        Counts {
            callbacks: self.callbacks.load(Ordering::Relaxed),
            samples: self.samples.load(Ordering::Relaxed),
            glitches: self.glitches.load(Ordering::Relaxed),
        }
    }

    /// The counts, once `FIRST_SECONDS` have passed at `now`, and only the first time.
    pub(super) fn due(&self, now: Instant) -> Option<Counts> {
        if now.duration_since(self.opened) < FIRST_SECONDS
            || self.handed.swap(true, Ordering::Relaxed)
        {
            return None;
        }
        Some(self.counts())
    }
}
