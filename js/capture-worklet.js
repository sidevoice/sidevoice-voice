// The capture's AudioWorklet processor, `sidevoice-capture`: it takes the microphone at the context's rate, downmixes
// it to mono, resamples it to 16 kHz and posts it on its port in frames of 10 ms (160 samples), each a Float32Array
// whose buffer is transferred. Loaded with `audioWorklet.addModule`; it runs in the AudioWorkletGlobalScope, where
// `sampleRate` is the context's.

const TARGET_RATE = 16000;
const FRAME = TARGET_RATE / 100;

// A second-order low-pass (RBJ's cookbook, Butterworth Q) at `cutoff` Hz for `rate` Hz, run in place over samples.
class LowPass {
  constructor(cutoff, rate) {
    const w = (2 * Math.PI * cutoff) / rate;
    const alpha = Math.sin(w) / (2 * Math.SQRT1_2);
    const cos = Math.cos(w);
    const a0 = 1 + alpha;
    this.b0 = (1 - cos) / 2 / a0;
    this.b1 = (1 - cos) / a0;
    this.b2 = this.b0;
    this.a1 = (-2 * cos) / a0;
    this.a2 = (1 - alpha) / a0;
    this.x1 = this.x2 = this.y1 = this.y2 = 0;
  }

  run(samples) {
    for (let i = 0; i < samples.length; i++) {
      const x = samples[i];
      const y = this.b0 * x + this.b1 * this.x1 + this.b2 * this.x2 - this.a1 * this.y1 - this.a2 * this.y2;
      this.x2 = this.x1;
      this.x1 = x;
      this.y2 = this.y1;
      this.y1 = y;
      samples[i] = y;
    }
  }
}

class CaptureProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    // Source samples per output sample; the anti-aliasing filters (two in cascade, a fourth-order low-pass under the
    // 8 kHz Nyquist of the output) only when there is something to remove.
    this.step = sampleRate / TARGET_RATE;
    this.filters = this.step > 1 ? [0, 1].map(() => new LowPass(TARGET_RATE * 0.45, sampleRate)) : [];
    // Where the next output sample falls, in source samples after `previous`, the last sample of the block before.
    this.position = 0;
    this.previous = 0;
    this.frame = new Float32Array(FRAME);
    this.filled = 0;
  }

  process(inputs) {
    const channels = inputs[0];
    if (!channels || channels.length === 0) {
      return true;
    }
    const length = channels[0].length;
    const mono = new Float32Array(length);
    for (const channel of channels) {
      for (let i = 0; i < length; i++) {
        mono[i] += channel[i] / channels.length;
      }
    }
    for (const filter of this.filters) {
      filter.run(mono);
    }
    // Linear interpolation between `previous` (index -1) and the block's samples.
    while (this.position < length - 1) {
      const at = this.position;
      const index = Math.floor(at);
      const before = index < 0 ? this.previous : mono[index];
      const after = mono[index + 1];
      this.push(before + (after - before) * (at - index));
      this.position += this.step;
    }
    this.position -= length;
    this.previous = mono[length - 1];
    return true;
  }

  push(sample) {
    this.frame[this.filled++] = sample;
    if (this.filled === FRAME) {
      this.port.postMessage(this.frame, [this.frame.buffer]);
      this.frame = new Float32Array(FRAME);
      this.filled = 0;
    }
  }
}

registerProcessor("sidevoice-capture", CaptureProcessor);
