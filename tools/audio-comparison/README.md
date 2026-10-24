# Native audio comparison

This standalone package links both the former `webrtc-audio-processing = 2.1.0` (bundled C++) and
`sonora = 0.2.0`. It is deliberately outside the product dependency graph. The Sonora engine is compiled
from `src/echo/native.rs`; the C++ configuration is frozen from main
`e5bf68353473c5bd738b3fac9e1c1b509aa86a93`. Neither engine receives a delay hint.

## Reproduce (Linux)

Install Rust 1.98.1, a C/C++ compiler, meson, ninja, pkg-config, libclang development files, and
Rust's llvm-tools component. For example on Ubuntu:

```sh
sudo apt-get install build-essential meson ninja-build pkg-config libclang-dev
rustup component add llvm-tools
CARGO_TARGET_DIR=/tmp/target-l4 flock /tmp/cargo-build.lock cargo run --locked --release \
  --manifest-path tools/audio-comparison/Cargo.toml
```

Run on an otherwise idle machine. Keep the same compiler, machine and optimization settings when
comparing results. Rust uses Cargo's release profile; bundled C++ uses its upstream Meson
`debugoptimized` default (`-O2`), exactly as in the former product build. No audio is downloaded or
written. Building this comparison is optional and is never part of a product build, Windows CI,
or the wasm package. The normal product needs none of these C++ tools.

## Inputs

Both engines receive identical 16 kHz mono float samples, in 160-sample (10 ms) frames, render before
capture. Recorded speech is reused from [the existing fixtures](../../tests/fixtures/README.md),
which documents sources, CC-BY-4.0 attribution and SHA-256 checksums:

- Far-end: LibriSpeech dev-clean 1272-128104-0000, peak normalized to 0.7, looped for 60 seconds.
- Near-end: FLEURS es_419 test sentence 1770, peak normalized to 0.5, looped for 60 seconds.
- Echo: far-end convolved with three reflections: 60/75/95 ms at -6/-14/-20 dB. This is a simulated
  linear room, not a measured room impulse response.
- Noise: reproducible uniform white noise, xorshift32 seed `0x12345678`, amplitude +/-0.01
  (approximately -44.8 dBFS RMS). Speech is recorded; room and noise are synthesized.
- Timeline: 0–20 s echo only; 20–40 s echo + near-end speech + noise; 40–60 s echo + noise.
  Separate 60 s near-only and noise-only probes use silence as the render reference.

## Metrics and limits

- ERLE is `10 log10(sum(echo²) / sum(output²))` on seconds 10–20, after convergence and before
  double-talk. It is full-pipeline echo attenuation (including HPF/NS/AGC), not the internal linear
  filter's ERLE. Never label a ratio measured during double-talk as ERLE: its denominator includes
  wanted speech. Residual RMS in that echo-only interval is also printed.
- Added signal delay is the peak normalized cross-correlation lag (0–320 samples, 0–20 ms) of the
  near-only probe, evaluated on seconds 10–20. Resolution is one sample (0.0625 ms). This excludes
  the common 10 ms frame accumulation, device buffers and wall-clock computation time.
- CPU is thread CPU time for render + capture, divided by the number of 10 ms frames; report the
  median and range of nine complete 60 s repetitions. Constructor, input generation, output copying
  to a recording, and metric calculations are outside the timed loop. A stack frame copy and
  `black_box` are identical in both paths. Wall time per frame is printed separately because a
  shared host can deschedule the process.
- Noise attenuation and near-only gain are input/output energy ratios over seconds 10–60 of their
  respective probes. These include all enabled stages, including adaptive gain; they do not isolate
  the noise suppressor. Double-talk output RMS is diagnostic, not a speech-quality score.
- All stages use product settings: full AEC3 and full-band HPF; moderate NS; adaptive AGC2 with
  5 dB headroom, 50 dB maximum gain, 15 dB initial gain, 6 dB/s slew and -50 dBFS maximum noise;
  fixed digital gain 0 dB and input-volume control off. Sonora's different pipeline defaults are
  overridden to the previous 48 kHz internal limit and disabled multichannel flags.

The operator's unchanged gate is **MATCH** only when ERLE is within 1 dB of C++, added latency is
at most 10% worse, and CPU per frame is at most 10% worse. Otherwise report **DIFFERS**. This small
corpus does not establish perceptual quality, nonlinear-room performance, device latency, or
performance on another CPU/OS. macOS/Windows CI tests are build/functional checks, not performance
measurements. See the PR's **Comparison vs C++** table for this run and **Decision for the operator**
for its disposition.

## Linux run, 2026-10-24

Host: Linux x86_64, Intel Core i5-8259U @ 2.30 GHz (shared pod); Rust 1.98.1 release profile,
GCC 12.2.0 C++ at upstream `-O2 -g`, Zig 0.16.0/Clang 21 for C and final linking, Meson 1.12.1.
No `target-cpu=native` override. Dependencies are fixed by this package's lockfile.

| Engine | ERLE dB | Added signal delay ms | CPU µs/10 ms frame (median; min–max) | Residual echo dBFS | Noise attenuation dB | Near-only gain dB | Double-talk output dBFS |
|---|---:|---:|---:|---:|---:|---:|---:|
| C++ 2.1.0 | 46.421 | 13.875 | 119.685; 113.834–137.882 | -70.631 | 3.221 | 5.169 | -23.637 |
| Sonora 0.2.0 | 66.013 | 16.688 | 131.475; 124.092–141.118 | -90.222 | 3.995 | 4.153 | -24.164 |

**DIFFERS**: ERLE +19.592 dB (better attenuation, but outside the required ±1 dB band), estimated
signal delay +20.3%, CPU +9.9%. CPU is close to the 10% boundary and varied across repetitions;
this run does not establish a robust CPU margin. Median wall time per frame was 119.698 µs for
C++ and 131.489 µs for Sonora. Full-pipeline gain is 1.016 dB lower in the near-only probe.
The operator must decide whether the changed delay/gain and stronger suppression are acceptable;
these numbers do not authorize a merge or release.
