//! Signal metrics, independent of the processing engines. Equations and limits are in README.md.
use std::f64::consts::TAU;

#[cfg(test)]
#[path = "metrics/tests.rs"]
mod tests;

pub const RATE: usize = 16_000;

pub fn energy(x: &[f32]) -> f64 {
    x.iter().map(|&v| f64::from(v).powi(2)).sum()
}
pub fn db(ratio: f64) -> f64 {
    10.0 * ratio.max(1e-12).log10()
}
pub fn white_noise(count: usize, amplitude: f32) -> Vec<f32> {
    let mut rng = 0x12345678_u32;
    (0..count)
        .map(|_| {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            (rng as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32 * amplitude
        })
        .collect()
}

/// Peak normalized correlation. Supply a phase-matched reference, not untreated speech.
pub fn lag(reference: &[f32], output: &[f32], start: usize, count: usize) -> (usize, f64) {
    let x = &reference[start..start + count];
    let ex = energy(x);
    (0..=320)
        .map(|lag| {
            let y = &output[start + lag..start + lag + count];
            let dot: f64 = x
                .iter()
                .zip(y)
                .map(|(&x, &y)| f64::from(x) * f64::from(y))
                .sum();
            (lag, dot / (ex * energy(y)).sqrt().max(1e-30))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap()
}

/// HPF group delay -d(arg H)/d(omega), in ms, from the library's measured impulse response.
/// Central phase difference at +/-0.5 Hz; wrap the difference, not the absolute phases.
pub fn group_delay(impulse: &[f32], hz: f64) -> f64 {
    let phase = |freq: f64| {
        let (re, im) = impulse
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(re, im), (i, &v)| {
                let angle = -TAU * freq * i as f64 / RATE as f64;
                (
                    re + f64::from(v) * angle.cos(),
                    im + f64::from(v) * angle.sin(),
                )
            });
        im.atan2(re)
    };
    let diff = phase(hz + 0.5) - phase(hz - 0.5);
    let wrapped = diff.sin().atan2(diff.cos());
    -wrapped / TAU * 1000.0
}

pub struct Quality {
    pub snr: f64,
    pub si_sdr: f64,
    pub gain: f64,
}
/// SNR without level fitting, SI-SDR with one global projection, and the projection's gain.
/// Both signals are centered. No per-frame gain or time fitting can hide dropped syllables.
pub fn quality(reference: &[f32], output: &[f32]) -> Quality {
    assert_eq!(reference.len(), output.len());
    let mean = |x: &[f32]| x.iter().map(|&v| f64::from(v)).sum::<f64>() / x.len() as f64;
    let (mx, my) = (mean(reference), mean(output));
    let (mut xx, mut xy, mut raw_error) = (0.0, 0.0, 0.0);
    for (&x, &y) in reference.iter().zip(output) {
        let (x, y) = (f64::from(x) - mx, f64::from(y) - my);
        xx += x * x;
        xy += x * y;
        raw_error += (y - x).powi(2);
    }
    assert!(xx > 1e-20, "quality needs non-silent reference");
    let alpha = xy / xx;
    let error: f64 = reference
        .iter()
        .zip(output)
        .map(|(&x, &y)| (f64::from(y) - my - alpha * (f64::from(x) - mx)).powi(2))
        .sum();
    Quality {
        snr: db(xx / raw_error.max(1e-30)),
        si_sdr: db(alpha.powi(2) * xx / error.max(1e-30)),
        gain: db(alpha.powi(2)),
    }
}

/// Linearly interpolated quantile of sorted observations.
pub fn quantile(sorted: &[f64], p: f64) -> f64 {
    let at = p * (sorted.len() - 1) as f64;
    let low = at.floor() as usize;
    sorted[low] + (sorted[at.ceil() as usize] - sorted[low]) * at.fract()
}
