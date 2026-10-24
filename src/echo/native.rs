//! Sonora's WebRTC audio processing: AEC3 at 16 kHz mono in 10 ms frames, with automatic delay estimation,
//! the high-pass filter, moderate noise suppression and adaptive digital gain.
#![cfg(native)]

#[cfg(test)]
mod tests;

use sonora::config::{
    EchoCanceller as Aec, GainController2, HighPassFilter, MaxProcessingRate, NoiseSuppression,
    Pipeline,
};
use sonora::{AudioProcessing, Config, StreamConfig};

use crate::audio::FRAME;
use crate::turns::RATE;

/// The echo canceller of one call. Every capture frame must follow the playback frame of the same 10 ms.
#[derive(Debug)]
pub(crate) struct EchoCanceller(AudioProcessing);

impl EchoCanceller {
    /// AEC3 with the high-pass filter, noise suppression and adaptive gain.
    pub(crate) fn new() -> Result<Self, String> {
        Ok(Self::with(true))
    }

    /// AEC3 and the high-pass filter, with noise suppression and gain when `cleanup` says so.
    fn with(cleanup: bool) -> Self {
        let stream = StreamConfig::new(RATE, 1);
        Self(
            AudioProcessing::builder()
                .capture_config(stream)
                .render_config(stream)
                .config(Config {
                    // Match the previous C++ configuration, including defaults that differ in Sonora.
                    pipeline: Pipeline {
                        maximum_internal_processing_rate: MaxProcessingRate::Rate48kHz,
                        multi_channel_render: false,
                        multi_channel_capture: false,
                        ..Default::default()
                    },
                    echo_canceller: Some(Aec::default()),
                    high_pass_filter: Some(HighPassFilter::default()),
                    noise_suppression: cleanup.then(NoiseSuppression::default),
                    gain_controller2: cleanup.then(|| GainController2 {
                        adaptive_digital: Some(Default::default()),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .build(),
        )
    }

    /// Takes 10 ms of what the speaker played.
    pub(crate) fn render(&mut self, frame: &[f32; FRAME]) {
        let mut output = [0.0; FRAME];
        self.0
            .process_render_f32(&[frame], &mut [&mut output])
            .expect("16 kHz mono render format is valid");
    }

    /// Takes 10 ms of what the microphone heard and leaves it without the echo.
    pub(crate) fn capture(&mut self, frame: &mut [f32; FRAME]) {
        let input = *frame;
        self.0
            .process_capture_f32(&[&input], &mut [frame])
            .expect("16 kHz mono capture format is valid");
    }

    /// Starts over, after the microphone or the speaker changed, preserving the cleanup settings.
    pub(crate) fn reinitialize(&mut self) {
        let config = self.0.config().clone();
        let stream = StreamConfig::new(RATE, 1);
        self.0 = AudioProcessing::builder()
            .capture_config(stream)
            .render_config(stream)
            .config(config)
            .build();
    }
}
