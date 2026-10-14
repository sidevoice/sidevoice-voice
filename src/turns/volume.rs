//! How loud the microphone is, as Pipecat measures it for its voice activity detector (`pipecat-ai` 1.10.0,
//! `AudioVolumeTracker` and `VADAnalyzer._get_smoothed_volume`): the integrated loudness of the last 400 ms (ITU-R
//! BS.1770: K-weighted, one gating block, the absolute gate at −70 LUFS, to the 0.1 LU of libloudness's histogram),
//! from −110 LUFS (0) to −10 LUFS (1), smoothed exponentially window by window. It reads 0 until 400 ms have been
//! heard, and for audio under the gate.

use std::collections::VecDeque;

use super::RATE;

/// The audio loudness is measured over: one BS.1770 gating block.
const BLOCK: usize = RATE as usize * 4 / 10;
/// The weight of the newest window in the smoothed volume.
const SMOOTHING: f32 = 0.2;
/// The loudness that reads 0, and the one that reads 1, in LUFS.
const FLOOR_LUFS: f64 = -110.0;
const CEILING_LUFS: f64 = -10.0;
/// Under it, a block is not measured at all (BS.1770's absolute gate).
const GATE_LUFS: f64 = -70.0;
/// The width and number of libloudness's histogram bins, from the gate up: a block reads as its bin's centre.
const BIN_LU: f64 = 0.1;
const BINS: f64 = 1_000.0;

/// The smoothed volume, from 0 to 1, and the audio it is measured on.
#[derive(Debug, Clone)]
pub(crate) struct Volume {
    block: VecDeque<f32>,
    weighting: KWeighting,
    value: f32,
}

impl Default for Volume {
    fn default() -> Self {
        Self {
            block: VecDeque::with_capacity(BLOCK),
            weighting: KWeighting::new(f64::from(RATE)),
            value: 0.0,
        }
    }
}

impl Volume {
    /// Takes the next window of samples (from −1 to 1) and returns the new smoothed volume.
    pub(crate) fn take(&mut self, pcm: &[f32]) -> f32 {
        self.block.extend(pcm);
        let excess = self.block.len().saturating_sub(BLOCK);
        self.block.drain(..excess);
        let volume = if self.block.len() == BLOCK {
            normalised(self.weighting.loudness(&self.block))
        } else {
            0.0
        };
        self.value += SMOOTHING * (volume - self.value);
        self.value
    }
}

/// A loudness in LUFS as a volume from 0 to 1; below the gate (−∞) it is 0.
fn normalised(lufs: f64) -> f32 {
    ((lufs - FLOOR_LUFS) / (CEILING_LUFS - FLOOR_LUFS)).clamp(0.0, 1.0) as f32
}

/// BS.1770's K-weighting at one rate: its high shelf and its high pass as one fourth-order filter, with the
/// coefficients libloudness derives for any rate (`k-filter.cpp`).
#[derive(Debug, Clone, Copy)]
struct KWeighting {
    b: [f64; 5],
    a: [f64; 5],
}

impl KWeighting {
    fn new(rate: f64) -> Self {
        let f0 = 1_681.974_450_955_533;
        let gain_db = 3.999_843_853_973_347;
        let vh = 10f64.powf(gain_db / 20.0);
        let vb = vh.powf(0.499_666_774_154_541_6);
        let q = 0.707_175_236_955_419_6;
        let k = (std::f64::consts::PI * f0 / rate).tan();
        let a0 = 1.0 + k / q + k * k;
        let pb = [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ];
        let pa = [1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0];
        let rb = [1.0, -2.0, 1.0];
        let f0 = 38.135_470_876_024_44;
        let q = 0.500_327_037_323_877_3;
        let k = (std::f64::consts::PI * f0 / rate).tan();
        let d = 1.0 + k / q + k * k;
        let ra = [1.0, 2.0 * (k * k - 1.0) / d, (1.0 - k / q + k * k) / d];
        Self {
            b: convolve(pb, rb),
            a: convolve(pa, ra),
        }
    }

    /// The loudness of `block`, filtered from rest, in LUFS: −∞ under the gate, else its histogram bin's centre.
    fn loudness(&self, block: &VecDeque<f32>) -> f64 {
        let (b, a) = (&self.b, &self.a);
        let mut v = [0.0f64; 5];
        let mut energy = 0.0;
        for &sample in block {
            v = [
                f64::from(sample) - a[1] * v[0] - a[2] * v[1] - a[3] * v[2] - a[4] * v[3],
                v[0],
                v[1],
                v[2],
                v[3],
            ];
            let out = b[0] * v[0] + b[1] * v[1] + b[2] * v[2] + b[3] * v[3] + b[4] * v[4];
            energy += out * out;
        }
        let lufs = 10.0 * (energy / block.len() as f64).log10() - 0.691;
        if lufs.is_nan() || lufs < GATE_LUFS {
            return f64::NEG_INFINITY;
        }
        let bin = ((lufs - GATE_LUFS) / BIN_LU).floor().min(BINS - 1.0);
        GATE_LUFS + bin * BIN_LU + BIN_LU / 2.0
    }
}

/// The product of two second-order polynomials.
fn convolve(x: [f64; 3], y: [f64; 3]) -> [f64; 5] {
    [
        x[0] * y[0],
        x[0] * y[1] + x[1] * y[0],
        x[0] * y[2] + x[1] * y[1] + x[2] * y[0],
        x[1] * y[2] + x[2] * y[1],
        x[2] * y[2],
    ]
}

#[cfg(test)]
pub(crate) fn loudness(pcm: &[f32]) -> f64 {
    KWeighting::new(f64::from(RATE)).loudness(&pcm.iter().copied().collect())
}
