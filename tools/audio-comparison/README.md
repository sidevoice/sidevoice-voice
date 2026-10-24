# Native audio comparison

This standalone package links the former `webrtc-audio-processing = 2.1.0` (bundled C++) and
`sonora = 0.2.0`. It is outside the product dependency graph. Full Sonora measurements compile
`src/echo/native.rs`; C++ uses the configuration frozen from pre-migration main `e5bf683`.
Neither full engine receives a delay hint. Product configuration is never changed by a probe.

## Reproduce

Use Rust 1.98.1, a C/C++ compiler, meson, ninja, pkg-config, libclang, and Rust's llvm-tools component.
On Ubuntu, install `build-essential meson ninja-build pkg-config libclang-dev`; on macOS, install
`meson ninja pkg-config llvm` with Homebrew and set `LIBCLANG_PATH=$(brew --prefix llvm)/lib`.
Then run:

```sh
rustup component add llvm-tools
CARGO_TARGET_DIR=/tmp/target-l4 flock /tmp/cargo-build.lock cargo xtask audio-comparison
```

On macOS, where `flock` is not installed by default, run `cargo xtask audio-comparison` directly.
This command checks formatting, lints, tests the metric library, and runs the release benchmark.
Rust uses the normal release profile; bundled C++ keeps upstream Meson `debugoptimized` (`-O2`).
No `target-cpu=native` override. Run without other heavy work. Compare engines on the same host;
absolute timings across different machines are not an engine comparison.

The existing `ci` workflow also has a manual `comparison: true` input. It runs this command on
Ubuntu 24.04 and macOS 15, uploads the complete output with commit/host/compiler metadata and all
paired timings, and skips ordinary product CI for that dispatch. Ordinary PR CI never builds C++.

## Inputs and configuration

Both full engines receive identical 16 kHz mono float samples, in 160-sample (10 ms) frames,
render before capture. [Existing fixtures](../../tests/fixtures/README.md) document source,
CC-BY-4.0 attribution and SHA-256 hashes; no audio is downloaded or added:

- Far-end: LibriSpeech dev-clean 1272-128104-0000, peak 0.7, looped for 60 s.
- Near-end: FLEURS es_419 sentence 1770, peak 0.5, looped for 60 s.
- Echo: far-end reflections at 60/75/95 ms, gains -6/-14/-20 dB (simulated linear room).
- Noise: xorshift32 seed `0x12345678`, uniform +/-0.01, about -44.8 dBFS RMS.
- Timeline: 0–20 s echo only; 20–40 s echo + near-end + noise; 40–60 s echo + noise.
  Separate 60 s near-only and noise-only probes have silence as render reference.

Full product settings: AEC3, full-band HPF, moderate NS, adaptive AGC2 (5 dB headroom, 50 dB max
and 15 dB initial gain, 6 dB/s slew, -50 dBFS maximum noise), fixed gain 0 dB and input-volume
control off. Sonora's rate/multichannel defaults are overridden to the former settings. These
settings match, but implementations differ: Sonora's 16 kHz HPF has three biquads; C++ has one.

## Buffering and HPF group delay

The old speech/unfiltered-input correlation was **not a buffering measurement** and is superseded.
HPF phase changes the correlation peak, particularly on low-frequency speech.

The revised probe is 16 s of deterministic broadband noise at +/-0.1, with silent render. Each
full engine is compared with its **own HPF-only reference pipeline** (all other stages disabled).
At 16 kHz mono this reference has no resampling or split-band buffering. The same HPF phase is
present in reference and output. Peak normalized correlation over lags 0–320 samples is measured
in three independent 1 s windows starting at 10, 12 and 14 s. All three must agree, correlate
above 0.8 and avoid the search boundary; otherwise the harness refuses to report a verdict.
This is a phase-matched broadband estimate of buffering, resolution 0.0625 ms. It includes the
processing stages' queues, excluding common frame accumulation, device buffers and CPU time.
No settings are disabled in the measured full product pipeline.

HPF group delay is measured separately from each HPF-only pipeline's 1 s impulse response:
`-d(arg H)/d(omega)` using central phase differences at +/-0.5 Hz, with phase wrapping handled.
Report 100/300/1000/3000 Hz; **there is no frequency-independent HPF delay to add to buffering**.
Tests recover a known 224-sample delay and distinguish a filtered reference from buffering.

## Near-end preservation

Report [SI-SDR](https://www.merl.com/publications/TR2019-013), ordinary SNR, and projected gain.
The equations are small scalar sums implemented/tested here; no perceptual-score library is used.
For centered reference `s` and delayed output `y`, one scalar for the entire interval is
`a = dot(s,y)/dot(s,s)`. SI-SDR is `10 log10(||a*s||² / ||y-a*s||²)`; ordinary SNR is
`10 log10(||s||² / ||y-s||²)`, with **no gain fitting**; projected gain is `20 log10(abs(a))`.
Silence gives -120 dB SI-SDR/gain rather than a favorable or undefined score.

Near-only uses seconds 10–20; double-talk uses the entire 20–40 s interval, including its onset.
Align using the independently measured buffering delay; **no speech-lag search or per-frame gain
fit**. The HPF-matched clean near-end reference removes the intended filter's phase/color from
AEC/NS/gain assessment. Its double-talk reference is gated at exactly 20 s before HPF filtering.
Also report SI-SDR against the **unfiltered** clean reference so HPF coloration is not hidden.
Residual echo, noise, dropped speech and distortion all enter the quality error. SI-SDR alone
would hide uniform attenuation, hence the mandatory SNR/gain rows and attenuation/dropout/silence
tests. These objective waveform measures are not intelligibility or perceptual listening scores.
The product's existing preservation test now runs with cleanup both off and on.

## ERLE and CPU

ERLE remains `10 log10(sum(echo²)/sum(output²))` on seconds 10–20 after convergence and before
near-end speech starts. It includes all product stages; it is not internal linear-filter ERLE.
Never interpret a double-talk power ratio as ERLE. Noise-only attenuation uses seconds 10–60.

CPU measurements use twelve paired iterations. Each engine is constructed afresh, processes the
first 10 s **before timing**, then continues on seconds 10–60 with that same state. Order alternates
C++/Sonora then Sonora/C++, six times each. Thread CPU time covers render+capture; constructor,
warmup, allocation of input recordings, metric calculations and destructor are excluded. The same
stack copy, dynamic dispatch and `black_box` apply to both. Report every pair, median, interpolated
quartiles, min/max, whether min/max ranges overlap, and median wall time separately. Timing
spread is empirical variability, not a confidence interval or proof of equivalence.

The unchanged gate is **MATCH** only for ERLE within ±1 dB, buffering latency at most 10% worse,
and median CPU per frame at most 10% worse; otherwise **DIFFERS**. HPF group delay is reported
separately by frequency, and near-end quality is additional operator evidence with no invented
acceptance threshold. This small corpus does not establish nonlinear-room behavior, device latency,
or perceptual quality. The PR's **Comparison vs C++** gives the dated Linux/macOS results and linked
run artifacts; **Decision for the operator** owns their disposition. Earlier speech-delay and CPU
figures are superseded by this methodology, not combined with it.
