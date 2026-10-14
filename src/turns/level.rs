//! How loud the microphone looks, for a meter: each window's RMS in dBFS, from −60 (0) to 0 (1), smoothed
//! exponentially. It decides nothing (what counts as speech is the volume's, `volume`).

/// The weight of the newest window in the smoothed level.
const SMOOTHING: f32 = 0.2;

/// The smoothed level, from 0 to 1.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Level {
    value: f32,
}

impl Level {
    /// Takes a window of samples (from −1 to 1).
    pub(crate) fn take(&mut self, pcm: &[f32]) {
        self.value += SMOOTHING * (instant(pcm) - self.value);
    }

    pub(crate) fn value(self) -> f32 {
        self.value
    }
}

/// The level of one window, unsmoothed.
pub(crate) fn instant(pcm: &[f32]) -> f32 {
    if pcm.is_empty() {
        return 0.0;
    }
    let mean = pcm.iter().map(|s| s * s).sum::<f32>() / pcm.len() as f32;
    let rms = mean.sqrt();
    if rms < 1e-9 {
        return 0.0;
    }
    ((20.0 * rms.log10()).clamp(-60.0, 0.0) + 60.0) / 60.0
}
