//! Speech heard over sounding playback, accumulated in a sliding window: windows of capture that qualify as speech
//! (the detector holds them for speech and they clear the playing bar) count towards an interruption wherever they fall
//! in the last stretch of audio, so short bursts with pauses between them add up where one unbroken run would never
//! reach the minimum. The window starts at the first window the detector holds for speech, however soft, so the
//! opening word of an interruption that is under the playing bar is kept; audio before that is forgotten at once, so
//! playback the detector never took for speech never lingers. When playback stops, or starts, in the middle of an
//! interruption, what was heard on one side is credited to the other (`credit`, `Overlap::speaking`).

use std::collections::VecDeque;

/// The window the minimum speech has to fall in, in samples: `factor` times the minimum (2.5 by default, so that speech
/// makes up two fifths of it).
pub(super) fn window(min_speech: usize, factor: f32) -> usize {
    (min_speech as f64 * f64::from(factor)).round() as usize
}

/// What one window of capture was.
#[derive(Debug, Clone, Copy)]
struct Window {
    samples: usize,
    /// The detector holds it for speech.
    held: bool,
    /// It is held for speech and clears the bar.
    qualified: bool,
}

/// The windows of capture since the first one the detector held for speech still inside the sliding window.
#[derive(Debug, Default)]
pub(super) struct Overlap {
    /// The windows in order; the first is always one held for speech.
    windows: VecDeque<Window>,
    /// The samples of the qualified windows.
    speech: usize,
    /// The samples of all of them.
    total: usize,
}

impl Overlap {
    /// Takes the next window of `samples`, with whether the detector `held` it for speech and whether it `qualified`
    /// (held, and over the bar), with the sliding window `limit` samples long.
    pub(super) fn take(&mut self, samples: usize, held: bool, qualified: bool, limit: usize) {
        self.windows.push_back(Window {
            samples,
            held,
            qualified,
        });
        self.total += samples;
        if qualified {
            self.speech += samples;
        }
        while let Some(&front) = self.windows.front() {
            if self.total <= limit && front.held {
                break;
            }
            self.windows.pop_front();
            self.total -= front.samples;
            if front.qualified {
                self.speech -= front.samples;
            }
        }
    }

    /// The qualified samples in the window.
    pub(super) fn speech(&self) -> usize {
        self.speech
    }

    /// Whether the last window qualified: speech goes on up to now.
    pub(super) fn speaking(&self) -> bool {
        self.windows.back().is_some_and(|window| window.qualified)
    }

    /// The samples from the first window held for speech to the end.
    pub(super) fn total(&self) -> usize {
        self.total
    }

    /// The samples a turn opened now keeps: from the first qualified window to the end, and before it the audio held
    /// for speech, at most `pre_roll` samples of it.
    pub(super) fn kept(&self, pre_roll: usize) -> usize {
        let lead: usize = self
            .windows
            .iter()
            .take_while(|window| !window.qualified)
            .map(|window| window.samples)
            .sum();
        self.total - lead + lead.min(pre_roll)
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
}
