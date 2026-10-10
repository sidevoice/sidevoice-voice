//! The capture's way to the call, on the module's audio thread: the microphone's samples to mono at 16 kHz in 10 ms
//! frames, each passed through the echo canceller after the frame of what the speaker played in the same 10 ms.
//!
//! The reference is what the output callback took, at the moment it took it, at the output's rate; it goes through
//! the same conversion. Capture and reference go in lockstep: each capture frame takes the oldest reference frame,
//! or silence when the speaker had nothing; a reference that runs ahead (two devices, two clocks) is kept to half a
//! second, the rest dropped from its oldest end. What delay is left, AEC3 estimates.

use std::collections::VecDeque;

use crate::audio::{downmix, Framer, Resampler, FRAME};
use crate::echo::EchoCanceller;
use crate::turns::RATE;

/// The most reference frames kept ahead of the capture.
const MAX_AHEAD: usize = 50;

/// From the devices' samples to clean 16 kHz frames.
pub(crate) struct Pipeline {
    channels: usize,
    capture: Resampler,
    capture_frames: Framer,
    render: Resampler,
    render_frames: Framer,
    ahead: VecDeque<[f32; FRAME]>,
    echo: EchoCanceller,
    scratch: Vec<f32>,
}

impl Pipeline {
    /// A pipeline for a microphone of `capture_rate` and `channels`, and a speaker of `render_rate`.
    pub(crate) fn new(
        capture_rate: u32,
        channels: usize,
        render_rate: u32,
    ) -> Result<Self, String> {
        Ok(Self {
            channels,
            capture: Resampler::new(capture_rate, RATE),
            capture_frames: Framer::default(),
            render: Resampler::new(render_rate, RATE),
            render_frames: Framer::default(),
            ahead: VecDeque::new(),
            echo: EchoCanceller::new()?,
            scratch: Vec::new(),
        })
    }

    /// Takes what the speaker played: mono samples at its rate.
    pub(crate) fn render(&mut self, played: &[f32]) {
        self.scratch.clear();
        self.render.process(played, &mut self.scratch);
        self.ahead.extend(self.render_frames.push(&self.scratch));
        let excess = self.ahead.len().saturating_sub(MAX_AHEAD);
        self.ahead.drain(..excess);
    }

    /// Takes what the microphone heard (interleaved, at its rate) and returns every frame it completed, clean.
    pub(crate) fn capture(&mut self, heard: &[f32]) -> Vec<[f32; FRAME]> {
        let mut mono = Vec::with_capacity(heard.len() / self.channels.max(1));
        downmix(heard, self.channels, &mut mono);
        self.scratch.clear();
        self.capture.process(&mono, &mut self.scratch);
        let mut frames = self.capture_frames.push(&self.scratch);
        for frame in &mut frames {
            let reference = self.ahead.pop_front().unwrap_or([0.0; FRAME]);
            self.echo.render(&reference);
            self.echo.capture(frame);
        }
        frames
    }

    /// Starts the echo canceller over, and forgets the reference ahead: a device changed.
    pub(crate) fn reinitialize(&mut self) {
        self.echo.reinitialize();
        self.ahead.clear();
        self.render_frames.clear();
    }
}
