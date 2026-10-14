// The rendered speaker meter. This processor passes every channel through unchanged and reports mono 16 kHz
// samples after the playback gains (including fades and gaps). Rust applies the call's shared meter and cadence.
const TARGET_RATE = 16000;
const FRAME = 160;

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

class PlaybackProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.rate = sampleRate;
    this.step = sampleRate / TARGET_RATE;
    this.filters = this.step > 1 ? [0, 1].map(() => new LowPass(TARGET_RATE * 0.45, sampleRate)) : [];
    this.position = 0;
    this.previous = 0;
    this.frame = new Float32Array(FRAME);
    this.filled = 0;
    this.frameStart = 0;
    this.frameEnd = 0;
    this.outputFrame = 0;
    // A stalled page keeps the newest unposted frame, replacing obsolete telemetry.
    this.inFlight = 0;
    this.latest = null;
    this.resetAt = null;
    this.port.onmessage = ({ data }) => {
      if (data === "ack") {
        this.inFlight = Math.max(0, this.inFlight - 1);
        if (this.latest) {
          const latest = this.latest;
          this.latest = null;
          this.report(latest);
        }
      } else if (data?.type === "fence") {
        // Port messages are ordered: all reports posted before this processor boundary arrive before the marker.
        // An unposted frame is obsolete telemetry, so a fence discards it.
        this.latest = null;
        this.port.postMessage({ type: "fence", id: data.id });
      } else if (data?.type === "reset") {
        this.resetAt = Math.max(this.resetAt ?? 0, data.frame);
        this.port.postMessage({ type: "reset", id: data.id });
      }
    };
  }

  process(inputs, outputs) {
    const input = inputs[0] || [];
    const output = outputs[0] || [];
    const length = output[0]?.length ?? input[0]?.length ?? 0;
    if (!length) return true;
    const blockStart = typeof currentFrame === "number" ? currentFrame : this.outputFrame;
    this.outputFrame = blockStart + length;
    if (this.resetAt !== null && blockStart >= this.resetAt) {
      this.filters = this.step > 1 ? [0, 1].map(() => new LowPass(TARGET_RATE * 0.45, this.rate)) : [];
      this.position = 0;
      this.previous = 0;
      this.frame = new Float32Array(FRAME);
      this.filled = 0;
      this.latest = null;
      this.resetAt = null;
    }
    for (let channel = 0; channel < output.length; channel++) {
      output[channel].set(input[channel] ?? input[0] ?? new Float32Array(length));
    }
    const mono = new Float32Array(length);
    for (const channel of input) {
      for (let i = 0; i < length; i++) mono[i] += channel[i] / input.length;
    }
    for (const filter of this.filters) filter.run(mono);
    while (this.position < length - 1) {
      const index = Math.floor(this.position);
      const before = index < 0 ? this.previous : mono[index];
      const after = mono[index + 1];
      this.push(before + (after - before) * (this.position - index), blockStart + this.position);
      this.position += this.step;
    }
    this.position -= length;
    this.previous = mono[length - 1];
    return true;
  }

  report(frame) {
    if (this.inFlight < 4) {
      this.port.postMessage(frame, [frame.samples.buffer]);
      this.inFlight++;
    } else {
      this.latest = frame;
    }
  }

  push(sample, position) {
    if (this.filled === 0) this.frameStart = position;
    this.frameEnd = position + 1;
    this.frame[this.filled++] = sample;
    if (this.filled === FRAME) {
      this.report({ samples: this.frame, startFrame: this.frameStart, endFrame: this.frameEnd });
      this.frame = new Float32Array(FRAME);
      this.filled = 0;
    }
  }
}

registerProcessor("sidevoice-playback", PlaybackProcessor);
