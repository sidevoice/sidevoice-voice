use super::*;

#[test]
fn phase_matched_reference_recovers_buffering_after_a_filter() {
    let input = white_noise(4 * RATE, 0.1);
    let mut filtered = input.clone();
    for i in 1..input.len() {
        filtered[i] = input[i] - 0.9 * input[i - 1];
    }
    let mut output = vec![0.0; input.len()];
    output[224..].copy_from_slice(&filtered[..input.len() - 224]);
    let (delay, correlation) = lag(&filtered, &output, RATE, RATE);
    assert_eq!(delay, 224);
    assert!(correlation > 0.999999);
}

#[test]
fn group_delay_recovers_known_delay_without_phase_wrap_errors() {
    let mut impulse = vec![0.0; RATE];
    impulse[224] = 1.0;
    for hz in [100.0, 300.0, 1000.0, 3000.0] {
        assert!((group_delay(&impulse, hz) - 14.0).abs() < 1e-8);
    }
}

#[test]
fn quality_exposes_attenuation_dropouts_and_silence() {
    let input = white_noise(RATE, 0.1);
    let attenuated: Vec<f32> = input.iter().map(|v| v * 0.1).collect();
    let score = quality(&input, &attenuated);
    assert!((score.gain + 20.0).abs() < 1e-5);
    assert!(score.si_sdr > 100.0);
    assert!(score.snr < 1.0); // SI-SDR alone would hide this attenuation.
    let mut dropped = input.clone();
    dropped[RATE / 2..].fill(0.0);
    assert!(quality(&input, &dropped).si_sdr < 1.0);
    assert_eq!(quality(&input, &vec![0.0; RATE]).si_sdr, -120.0);
}

#[test]
fn median_uses_both_middle_observations() {
    assert_eq!(quantile(&[1.0, 2.0, 3.0, 4.0], 0.5), 2.5);
}
