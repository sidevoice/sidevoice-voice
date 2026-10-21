//! Whether the person is speaking, window by window, as Pipecat's voice activity detector decides it (`pipecat-ai`
//! 1.10.0, `VADAnalyzer` and `VADController`): a window is speech when the detector's confidence and the volume both
//! clear their bars; speech starts once `start` of it has gone by without a break, and stops once `stop` without
//! any. In between, a window of the other kind only interrupts the count: a breath inside speech, or a click inside
//! silence, changes nothing. Durations become counts of windows as Pipecat rounds them (half to even).

use super::RATE;

/// A change of whether the person is speaking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    Started,
    Stopped,
}

/// Where the detector is, with the windows counted towards the next state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum State {
    #[default]
    Quiet,
    Starting(u32),
    Speaking,
    Stopping(u32),
}

/// Speech and silence out of each window's decision.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Activity {
    state: State,
}

impl Activity {
    /// Takes one window of `samples` that is speech or not: whether speech started or stopped with it, after
    /// `start_ms` of speech or `stop_ms` of silence.
    pub(crate) fn take(
        &mut self,
        speech: bool,
        samples: usize,
        start_ms: u32,
        stop_ms: u32,
    ) -> Option<Change> {
        self.state = match (self.state, speech) {
            (State::Quiet, true) => State::Starting(1),
            (State::Starting(count), true) => State::Starting(count + 1),
            (State::Starting(_), false) => State::Quiet,
            (State::Speaking | State::Stopping(_), true) => State::Speaking,
            (State::Speaking, false) => State::Stopping(1),
            (State::Stopping(count), false) => State::Stopping(count + 1),
            (State::Quiet, false) => State::Quiet,
        };
        match self.state {
            State::Starting(count) if count >= windows(start_ms, samples) => {
                self.state = State::Speaking;
                Some(Change::Started)
            }
            State::Stopping(count) if count >= windows(stop_ms, samples) => {
                self.state = State::Quiet;
                Some(Change::Stopped)
            }
            _ => None,
        }
    }

    /// The windows of speech counted towards starting, none unless speech is starting.
    pub(crate) fn starting(self) -> usize {
        match self.state {
            State::Starting(count) => count as usize,
            _ => 0,
        }
    }

    /// Starts counting from `windows` windows of speech already heard.
    pub(crate) fn credit(&mut self, windows: usize) {
        self.state = match u32::try_from(windows) {
            Ok(count) if count > 0 => State::Starting(count),
            _ => State::Quiet,
        };
    }

    /// Whether speech is going on and has not started to stop: the last window was speech, or the one that resumed it.
    pub(crate) fn speaking(self) -> bool {
        self.state == State::Speaking
    }

    /// Back to quiet, with nothing counted.
    pub(crate) fn reset(&mut self) {
        self.state = State::Quiet;
    }
}

/// How many windows of `samples` make `ms`, rounded half to even.
pub(super) fn windows(ms: u32, samples: usize) -> u32 {
    let window_s = samples.max(1) as f64 / f64::from(RATE);
    (f64::from(ms) / 1000.0 / window_s).round_ties_even() as u32
}
