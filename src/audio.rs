//! Audio between the devices and the call: interleaved device samples to mono, a streaming resampler from any rate
//! to any other, and 10 ms frames at 16 kHz, the unit the echo canceller works in.

#[cfg(test)]
mod tests;

/// Samples in one 10 ms frame at 16 kHz.
pub(crate) const FRAME: usize = 160;

/// Appends the mono mix of `interleaved` (`channels` samples per frame) to `out`: the mean of its channels.
pub(crate) fn downmix(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    if channels <= 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    out.extend(
        interleaved
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32),
    );
}

/// A streaming resampler: linear interpolation, after a fourth-order Butterworth low-pass below the new Nyquist
/// frequency when the rate goes down. It keeps its state between blocks, so a stream cut in blocks of any size comes
/// out as if it were resampled whole.
#[derive(Debug)]
pub(crate) struct Resampler {
    /// Input samples per output sample.
    step: f64,
    /// Where the next output sample sits, in input samples from the start of the next block (−1 is `previous`).
    position: f64,
    previous: f32,
    filter: Option<[Biquad; 2]>,
}

impl Resampler {
    pub(crate) fn new(from: u32, to: u32) -> Self {
        let filter = (to < from).then(|| {
            let cutoff = 0.45 * f64::from(to);
            [
                Biquad::low_pass(f64::from(from), cutoff, 0.541_196_1),
                Biquad::low_pass(f64::from(from), cutoff, 1.306_563),
            ]
        });
        Self {
            step: f64::from(from) / f64::from(to),
            position: 0.0,
            previous: 0.0,
            filter,
        }
    }

    /// Appends `input`, resampled, to `out`.
    pub(crate) fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        let filtered;
        let input = match &mut self.filter {
            None => input,
            Some(filter) => {
                filtered = input
                    .iter()
                    .map(|&sample| filter[1].take(filter[0].take(sample)))
                    .collect::<Vec<_>>();
                &filtered
            }
        };
        let last = (input.len() - 1) as f64;
        while self.position < last {
            let index = self.position.floor();
            let fraction = (self.position - index) as f32;
            let here = if index < 0.0 {
                self.previous
            } else {
                input[index as usize]
            };
            let next = input[(index + 1.0) as usize];
            out.push(here + (next - here) * fraction);
            self.position += self.step;
        }
        self.position -= input.len() as f64;
        self.previous = input[input.len() - 1];
    }
}

/// One second-order section (RBJ's cookbook), in direct form I.
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    x: [f32; 2],
    y: [f32; 2],
}

impl Biquad {
    fn low_pass(rate: f64, cutoff: f64, q: f64) -> Self {
        let w = 2.0 * std::f64::consts::PI * cutoff / rate;
        let alpha = w.sin() / (2.0 * q);
        let cos = w.cos();
        let a0 = 1.0 + alpha;
        let b1 = (1.0 - cos) / a0;
        Self {
            b: [(b1 / 2.0) as f32, b1 as f32, (b1 / 2.0) as f32],
            a: [(-2.0 * cos / a0) as f32, ((1.0 - alpha) / a0) as f32],
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }

    fn take(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

/// Cuts a stream into whole 10 ms frames, keeping the rest for the next samples.
#[derive(Debug, Default)]
pub(crate) struct Framer {
    pending: Vec<f32>,
}

impl Framer {
    /// Takes `samples` and returns every frame they completed, in order.
    pub(crate) fn push(&mut self, samples: &[f32]) -> Vec<[f32; FRAME]> {
        self.pending.extend_from_slice(samples);
        let whole = self.pending.len() / FRAME * FRAME;
        let frames = self.pending[..whole].as_chunks::<FRAME>().0.to_vec();
        self.pending.drain(..whole);
        frames
    }

    pub(crate) fn clear(&mut self) {
        self.pending.clear();
    }
}
