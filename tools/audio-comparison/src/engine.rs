//! Full product engines and isolated HPF reference paths.
use crate::{audio::FRAME, native::EchoCanceller};
use webrtc_audio_processing::{config as cpp, Config, Processor};

pub(crate) trait Engine {
    fn frame(&mut self, render: &[f32; FRAME], capture: &mut [f32; FRAME]);
}
impl Engine for EchoCanceller {
    fn frame(&mut self, render: &[f32; FRAME], capture: &mut [f32; FRAME]) {
        self.render(render);
        self.capture(capture);
    }
}
impl Engine for Processor {
    fn frame(&mut self, render: &[f32; FRAME], capture: &mut [f32; FRAME]) {
        self.analyze_render_frame([&render[..]]).unwrap();
        self.process_capture_frame([&mut capture[..]]).unwrap();
    }
}
fn cpp() -> Processor {
    let processor = Processor::new(16_000).unwrap();
    // Verbatim settings from src/echo/native.rs at the pre-migration main commit.
    processor.set_config(Config {
        echo_canceller: Some(cpp::EchoCanceller::Full {
            stream_delay_ms: None,
        }),
        high_pass_filter: Some(cpp::HighPassFilter::default()),
        noise_suppression: Some(cpp::NoiseSuppression::default()),
        gain_controller: Some(cpp::GainController::GainController2(cpp::GainController2 {
            adaptive_digital: Some(Default::default()),
            ..Default::default()
        })),
        ..Default::default()
    });
    processor
}

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Cpp,
    Sonora,
}
impl Kind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Cpp => "C++ 2.1.0",
            Self::Sonora => "Sonora 0.2.0",
        }
    }
    pub(crate) fn full(self) -> Box<dyn Engine> {
        match self {
            Self::Cpp => Box::new(cpp()),
            Self::Sonora => Box::new(EchoCanceller::new().unwrap()),
        }
    }
    // A separate reference path: use the library's real HPF, with every other stage disabled.
    // At 16 kHz mono there is no resampler or splitting filter in this path.
    pub(crate) fn hpf(self) -> Box<dyn Engine> {
        match self {
            Self::Cpp => {
                let processor = Processor::new(16_000).unwrap();
                processor.set_config(Config {
                    high_pass_filter: Some(cpp::HighPassFilter::default()),
                    ..Default::default()
                });
                Box::new(processor)
            }
            Self::Sonora => Box::new(
                sonora::AudioProcessing::builder()
                    .config(sonora::Config {
                        high_pass_filter: Some(sonora::config::HighPassFilter::default()),
                        ..Default::default()
                    })
                    .build(),
            ),
        }
    }
}
impl Engine for sonora::AudioProcessing {
    fn frame(&mut self, render: &[f32; FRAME], capture: &mut [f32; FRAME]) {
        let mut scratch = [0.0; FRAME];
        self.process_render_f32(&[render], &mut [&mut scratch])
            .unwrap();
        let input = *capture;
        self.process_capture_f32(&[&input], &mut [capture]).unwrap();
    }
}
