//! Recorded speech and deterministic room/noise inputs.
use std::path::Path;

fn speech(name: &str, peak: f32) -> Vec<f32> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name);
    let mut reader = hound::WavReader::open(path).unwrap();
    assert_eq!(reader.spec().sample_rate, 16_000);
    assert_eq!(reader.spec().channels, 1);
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|v| f32::from(v.unwrap()) / 32768.0)
        .collect();
    let max = samples.iter().fold(0.0_f32, |a, v| a.max(v.abs()));
    samples.into_iter().map(|v| v * peak / max).collect()
}
pub(crate) struct Inputs {
    pub(crate) render: Vec<f32>,
    pub(crate) echo: Vec<f32>,
    pub(crate) near: Vec<f32>,
    pub(crate) noise: Vec<f32>,
    pub(crate) mixed: Vec<f32>,
}
impl Inputs {
    pub(crate) fn new() -> Self {
        let far = speech("librispeech_mr_quilter.wav", 0.7);
        let person = speech("fleur_es_sample.wav", 0.5);
        let count = 60 * 16_000;
        let render: Vec<f32> = far.iter().copied().cycle().take(count).collect();
        let near: Vec<f32> = person.iter().copied().cycle().take(count).collect();
        let mut echo = vec![0.0; count];
        for (delay_ms, gain_db) in [(60, -6.0), (75, -14.0), (95, -20.0)] {
            let delay = delay_ms * 16;
            let gain = 10f32.powf(gain_db / 20.0);
            for i in delay..count {
                echo[i] += render[i - delay] * gain;
            }
        }
        let noise = audio_comparison::white_noise(count, 0.01);
        // 0–20s: echo alone; 20–40s: double-talk + noise; 40–60s: echo + noise.
        let mixed = (0..count)
            .map(|i| {
                echo[i]
                    + if i >= 20 * 16_000 { noise[i] } else { 0.0 }
                    + if (20 * 16_000..40 * 16_000).contains(&i) {
                        near[i]
                    } else {
                        0.0
                    }
            })
            .collect();
        Self {
            render,
            echo,
            near,
            noise,
            mixed,
        }
    }
}
