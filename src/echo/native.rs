//! WebRTC's audio processing, natively (`webrtc-audio-processing`, its C++ built with the crate): AEC3 at 16 kHz
//! mono in 10 ms frames, estimating the delay between the playback and its echo itself, with the high-pass filter,
//! moderate noise suppression and the adaptive digital gain, as the browser's `getUserMedia` constraints give the web
//! build. Its C++ does not build on Windows yet (sidevoice-core#89), and the web build links none.
#![cfg(native)]

#[cfg(test)]
mod tests;

use webrtc_audio_processing::config::{
    EchoCanceller as Aec, GainController, GainController2, HighPassFilter, NoiseSuppression,
};
use webrtc_audio_processing::{Config, Processor};

use crate::audio::FRAME;
use crate::turns::RATE;

/// The echo canceller of one call. Every capture frame must follow the playback frame of the same 10 ms.
#[derive(Debug)]
pub(crate) struct EchoCanceller(Processor);

impl EchoCanceller {
    /// AEC3 with the high-pass filter, noise suppression and the adaptive gain.
    pub(crate) fn new() -> Result<Self, String> {
        Self::with(true)
    }

    /// AEC3 and the high-pass filter, with noise suppression and the gain when `cleanup` says so.
    fn with(cleanup: bool) -> Result<Self, String> {
        let processor = Processor::new(RATE).map_err(|_| "echo-canceller-failed".to_owned())?;
        processor.set_config(Config {
            echo_canceller: Some(Aec::Full {
                stream_delay_ms: None,
            }),
            high_pass_filter: Some(HighPassFilter::default()),
            noise_suppression: cleanup.then(NoiseSuppression::default),
            gain_controller: cleanup.then(|| {
                GainController::GainController2(GainController2 {
                    adaptive_digital: Some(Default::default()),
                    ..Default::default()
                })
            }),
            ..Default::default()
        });
        Ok(Self(processor))
    }

    /// Takes 10 ms of what the speaker played.
    pub(crate) fn render(&self, frame: &[f32; FRAME]) {
        let _ = self.0.analyze_render_frame([&frame[..]]);
    }

    /// Takes 10 ms of what the microphone heard and leaves it without the echo.
    pub(crate) fn capture(&self, frame: &mut [f32; FRAME]) {
        let _ = self.0.process_capture_frame([&mut frame[..]]);
    }

    /// Starts over, after the microphone or the speaker changed.
    pub(crate) fn reinitialize(&self) {
        self.0.reinitialize();
    }
}
