//! Speech heard over sounding playback, accumulated in a sliding window: windows of capture that qualify as speech
//! count towards an interruption wherever they fall in the last stretch of audio, so short bursts with pauses between
//! them add up where one unbroken run would never reach the minimum. Audio that does not qualify and is older than the
//! first qualified window is forgotten at once, so a lone echo never lingers.

use std::collections::VecDeque;

/// The window the minimum speech has to fall in, in samples: two and a half times the minimum, so that speech makes up
/// two fifths of it.
pub(super) fn window(min_speech: usize) -> usize {
    min_speech * 5 / 2
}

/// The windows of capture since the first qualified one still inside the sliding window.
#[derive(Debug, Default)]
pub(super) struct Overlap {
    /// Each window's length in samples and whether it qualified; the first is always a qualified one.
    windows: VecDeque<(usize, bool)>,
    /// The samples of the qualified windows.
    speech: usize,
    /// The samples of all of them.
    total: usize,
}

impl Overlap {
    /// Takes the next window of `samples`, qualified or not, with the sliding window `limit` samples long.
    pub(super) fn take(&mut self, samples: usize, qualified: bool, limit: usize) {
        self.windows.push_back((samples, qualified));
        self.total += samples;
        if qualified {
            self.speech += samples;
        }
        while let Some(&(front, speech)) = self.windows.front() {
            if self.total <= limit && speech {
                break;
            }
            self.windows.pop_front();
            self.total -= front;
            if speech {
                self.speech -= front;
            }
        }
    }

    /// The qualified samples in the window.
    pub(super) fn speech(&self) -> usize {
        self.speech
    }

    /// The samples from the first qualified window to the end.
    pub(super) fn total(&self) -> usize {
        self.total
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
}
