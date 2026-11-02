//! When a stream of the call is opened again, and when that stops being tried (`Reopening`); and whether a stream
//! still runs at all (`Watch`).
//!
//! A stream is opened again for one of three causes. A device chosen anew is opened once: if it cannot be, that is
//! said at once, as before. A stream the system took away or invalidated (a route change: another output, a headset, a
//! car kit, the session's category or a route configuration changed; the media services reset) is tried again while the
//! route settles, for up to [`RECOVERY`], before it is a failure. A stream that stopped calling back without a word (iOS
//! stops the audio unit itself while the session is interrupted, or deactivated under a CallKit call, and may never
//! start it again) is tried again for as long as the call runs: the system gives the audio back when it decides, and
//! that is no failure.

#[cfg(test)]
mod tests;

use std::time::{Duration, Instant};

/// How long a stream may go without a single callback before it is taken as stopped under the call.
pub(super) const STALL: Duration = Duration::from_secs(2);
/// How long a stream the system took away is tried again before it is a failure.
pub(super) const RECOVERY: Duration = Duration::from_secs(10);
/// How long after a failed try the next one is made.
pub(super) const RETRY: Duration = Duration::from_millis(500);
/// How long after a failed try the next one is made for a stalled stream: the system is not giving the audio back yet.
pub(super) const STALLED_RETRY: Duration = Duration::from_secs(1);

/// Why a stream is opened again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cause {
    /// Another device was chosen, or the chosen one went away for the default one.
    Switched,
    /// The system took the device away or invalidated the stream.
    Rerouted,
    /// The stream stopped calling back.
    Stalled,
}

impl Cause {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Cause::Switched => "switched",
            Cause::Rerouted => "rerouted",
            Cause::Stalled => "stalled",
        }
    }
}

/// A stream to be opened again: since when, how many tries failed, and when the next one is due.
#[derive(Debug)]
pub(super) struct Reopening {
    pub(super) cause: Cause,
    since: Instant,
    next: Instant,
    pub(super) failures: u32,
}

impl Reopening {
    /// To be opened again for `cause`, from `now`: the first try is due at once.
    pub(super) fn new(cause: Cause, now: Instant) -> Self {
        Self {
            cause,
            since: now,
            next: now,
            failures: 0,
        }
    }

    /// Whether a try is due at `now`. While the system holds the audio (`held`) only a device chosen anew is tried.
    pub(super) fn due(&self, now: Instant, held: bool) -> bool {
        now >= self.next && (!held || self.cause == Cause::Switched)
    }

    /// The system holds the audio at `now`: whether this is still to be opened again. A stalled stream is not (it is
    /// waited for, and watched again once the audio is let go); the recovery of one taken away starts over, so that it
    /// has its whole time once the audio is let go.
    pub(super) fn held(&mut self, now: Instant) -> bool {
        match self.cause {
            Cause::Stalled => false,
            Cause::Rerouted => {
                self.since = now;
                self.next = now;
                true
            }
            Cause::Switched => true,
        }
    }

    /// A try failed at `now`: whether another one is made (when it is due), or the stream is given up.
    pub(super) fn failed(&mut self, now: Instant) -> bool {
        self.failures += 1;
        let again = match self.cause {
            Cause::Switched => false,
            Cause::Rerouted => now.duration_since(self.since) < RECOVERY,
            Cause::Stalled => true,
        };
        self.next = now
            + if self.cause == Cause::Stalled {
                STALLED_RETRY
            } else {
                RETRY
            };
        again
    }

    /// How long it has been tried, at `now`, in milliseconds.
    pub(super) fn elapsed_ms(&self, now: Instant) -> u128 {
        now.duration_since(self.since).as_millis()
    }
}

/// Whether a stream still calls back: the count of its callbacks, and when it last changed.
#[derive(Debug)]
pub(super) struct Watch {
    count: u64,
    changed: Instant,
}

impl Watch {
    /// A stream opened at `now`, that has not called back yet.
    pub(super) fn new(now: Instant) -> Self {
        Self {
            count: 0,
            changed: now,
        }
    }

    /// Whether the stream, which has called back `count` times by `now`, has gone [`STALL`] without a callback.
    pub(super) fn stalled(&mut self, count: u64, now: Instant) -> bool {
        if count != self.count {
            self.count = count;
            self.changed = now;
            return false;
        }
        now.duration_since(self.changed) >= STALL
    }
}
