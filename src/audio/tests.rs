use super::{downmix, Framer, Resampler, FRAME};

/// A sine of `hz` at `rate`, one second.
fn sine(hz: f32, rate: u32) -> Vec<f32> {
    (0..rate)
        .map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin() * 0.5)
        .collect()
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

#[test]
fn channels_are_averaged() {
    let mut out = Vec::new();
    downmix(&[1.0, 0.0, 0.5, 0.5], 2, &mut out);
    downmix(&[0.25], 1, &mut out);
    assert_eq!(out, [0.5, 0.5, 0.25]);
}

#[test]
fn a_stream_resampled_in_blocks_is_the_stream_resampled_whole() {
    let input = sine(440.0, 48_000);
    let mut whole = Vec::new();
    Resampler::new(48_000, 16_000).process(&input, &mut whole);
    let mut blocks = Vec::new();
    let mut resampler = Resampler::new(48_000, 16_000);
    for block in input.chunks(441) {
        resampler.process(block, &mut blocks);
    }
    assert_eq!(whole.len(), blocks.len());
    assert!(whole.iter().zip(&blocks).all(|(a, b)| (a - b).abs() < 1e-5));
    assert!((15_990..=16_000).contains(&whole.len()), "{}", whole.len());
}

#[test]
fn speech_frequencies_pass_and_those_above_the_new_nyquist_are_cut() {
    let mut low = Vec::new();
    Resampler::new(48_000, 16_000).process(&sine(1_000.0, 48_000), &mut low);
    assert!((rms(&low[4_000..]) - 0.5 / 2f32.sqrt()).abs() < 0.02);
    let mut high = Vec::new();
    Resampler::new(48_000, 16_000).process(&sine(12_000.0, 48_000), &mut high);
    assert!(rms(&high[4_000..]) < 0.02, "{}", rms(&high[4_000..]));
}

#[test]
fn going_up_keeps_the_signal() {
    let mut up = Vec::new();
    Resampler::new(24_000, 48_000).process(&sine(440.0, 24_000), &mut up);
    assert!((47_990..=48_000).contains(&up.len()));
    assert!((rms(&up) - 0.5 / 2f32.sqrt()).abs() < 0.01);
}

#[test]
fn frames_are_whole_and_the_rest_waits() {
    let mut framer = Framer::default();
    assert!(framer.push(&[0.0; FRAME - 1]).is_empty());
    let frames = framer.push(&[1.0; FRAME + 1]);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0][FRAME - 2], 0.0);
    assert_eq!(frames[0][FRAME - 1], 1.0);
    assert_eq!(framer.push(&[1.0; FRAME - 2]).len(), 1);
}
