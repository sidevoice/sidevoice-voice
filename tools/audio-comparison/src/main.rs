//! Reproducible comparison against the former bundled C++ pipeline. See ../README.md.
use cpu_time::ThreadTime;
use std::{hint::black_box, path::Path, time::Instant};

// Compile the actual product adapter, so the benchmark cannot silently drift from it.
mod audio {
    pub const FRAME: usize = 160;
}
mod turns {
    pub const RATE: u32 = 16_000;
}
#[rustfmt::skip] // Formatted with the product; its unit tests belong to the product crate.
#[path = "../../../src/echo/native.rs"]
mod native;
use audio::FRAME;
use native::EchoCanceller;
use webrtc_audio_processing::{config as cpp, Config, Processor};

trait Engine {
    fn frame(&mut self, render: &[f32; FRAME], capture: &mut [f32; FRAME]);
}
impl Engine for EchoCanceller {
    fn frame(&mut self, render: &[f32; FRAME], capture: &mut [f32; FRAME]) {
        self.render(render);
        self.capture(capture);
    }
}
impl Engine for Processor {
    fn frame(&mut self, render: &[f32; FRAME], capture: &mut [f32; FRAME]) {
        self.analyze_render_frame([&render[..]]).unwrap();
        self.process_capture_frame([&mut capture[..]]).unwrap();
    }
}
fn cpp() -> Processor {
    let processor = Processor::new(16_000).unwrap();
    // Verbatim settings from src/echo/native.rs at the pre-migration main commit.
    processor.set_config(Config {
        echo_canceller: Some(cpp::EchoCanceller::Full {
            stream_delay_ms: None,
        }),
        high_pass_filter: Some(cpp::HighPassFilter::default()),
        noise_suppression: Some(cpp::NoiseSuppression::default()),
        gain_controller: Some(cpp::GainController::GainController2(cpp::GainController2 {
            adaptive_digital: Some(Default::default()),
            ..Default::default()
        })),
        ..Default::default()
    });
    processor
}
fn speech(name: &str, peak: f32) -> Vec<f32> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name);
    let mut reader = hound::WavReader::open(path).unwrap();
    assert_eq!(reader.spec().sample_rate, 16_000);
    assert_eq!(reader.spec().channels, 1);
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|v| f32::from(v.unwrap()) / 32768.0)
        .collect();
    let max = samples.iter().fold(0.0_f32, |a, v| a.max(v.abs()));
    samples.into_iter().map(|v| v * peak / max).collect()
}
fn energy(values: &[f32]) -> f64 {
    values.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>()
}
fn db(ratio: f64) -> f64 {
    10.0 * ratio.max(1e-30).log10()
}
fn rms_db(values: &[f32]) -> f64 {
    db(energy(values) / values.len() as f64)
}
fn process(mut engine: impl Engine, render: &[f32], mic: &[f32]) -> Vec<f32> {
    let mut out = Vec::with_capacity(mic.len());
    for (render, mic) in render
        .as_chunks::<FRAME>()
        .0
        .iter()
        .zip(mic.as_chunks::<FRAME>().0)
    {
        let mut frame = *mic;
        engine.frame(render, &mut frame);
        assert!(frame.iter().all(|v| v.is_finite()));
        out.extend(frame);
    }
    out
}
struct Inputs {
    render: Vec<f32>,
    echo: Vec<f32>,
    near: Vec<f32>,
    noise: Vec<f32>,
    mixed: Vec<f32>,
}
impl Inputs {
    fn new() -> Self {
        let far = speech("librispeech_mr_quilter.wav", 0.7);
        let person = speech("fleur_es_sample.wav", 0.5);
        let count = 60 * 16_000;
        let render: Vec<f32> = far.iter().copied().cycle().take(count).collect();
        let near: Vec<f32> = person.iter().copied().cycle().take(count).collect();
        let mut echo = vec![0.0; count];
        for (delay_ms, gain_db) in [(60, -6.0), (75, -14.0), (95, -20.0)] {
            let delay = delay_ms * 16;
            let gain = 10f32.powf(gain_db / 20.0);
            for i in delay..count {
                echo[i] += render[i - delay] * gain;
            }
        }
        let mut rng = 0x12345678_u32;
        let noise: Vec<f32> = (0..count)
            .map(|_| {
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                (rng as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32 * 0.01
            })
            .collect();
        // 0–20s: echo alone; 20–40s: double-talk + noise; 40–60s: echo + noise.
        let mixed = (0..count)
            .map(|i| {
                echo[i]
                    + if i >= 20 * 16_000 { noise[i] } else { 0.0 }
                    + if (20 * 16_000..40 * 16_000).contains(&i) {
                        near[i]
                    } else {
                        0.0
                    }
            })
            .collect();
        Self {
            render,
            echo,
            near,
            noise,
            mixed,
        }
    }
}
fn delay(input: &[f32], output: &[f32]) -> f64 {
    // Peak normalized cross-correlation of a near-only speech probe, after 10 s warmup.
    let start = 10 * 16_000;
    let end = 20 * 16_000;
    let lag = (0..=320)
        .max_by(|&a, &b| {
            let score = |lag| {
                let x = &input[start..end];
                let y = &output[start + lag..end + lag];
                let dot: f64 = x
                    .iter()
                    .zip(y)
                    .map(|(x, y)| f64::from(*x) * f64::from(*y))
                    .sum();
                dot / (energy(x) * energy(y)).sqrt().max(1e-30)
            };
            score(a).total_cmp(&score(b))
        })
        .unwrap();
    lag as f64 / 16.0
}
struct Metrics {
    erle: f64,
    latency: f64,
    cpu: f64,
}
fn measure<E: Engine>(name: &str, new: impl Fn() -> E, input: &Inputs) -> Metrics {
    let output = process(new(), &input.render, &input.mixed);
    let erle =
        db(energy(&input.echo[10 * 16_000..20 * 16_000])
            / energy(&output[10 * 16_000..20 * 16_000]));
    let zero = vec![0.0; input.render.len()];
    let near = process(new(), &zero, &input.near);
    let noise = process(new(), &zero, &input.noise);
    let latency = delay(&input.near, &near);
    let gain = db(energy(&near[10 * 16_000..]) / energy(&input.near[10 * 16_000..]));
    let ns = db(energy(&input.noise[10 * 16_000..]) / energy(&noise[10 * 16_000..]));
    let mut cpu = vec![];
    let mut wall = vec![];
    // Each repetition starts fresh and sees the same samples. Creation and I/O are excluded.
    for _ in 0..9 {
        let mut engine = new();
        let start_cpu = ThreadTime::now();
        let start_wall = Instant::now();
        for (r, c) in input
            .render
            .as_chunks::<FRAME>()
            .0
            .iter()
            .zip(input.mixed.as_chunks::<FRAME>().0)
        {
            let mut frame = *c;
            engine.frame(black_box(r), black_box(&mut frame));
            black_box(frame);
        }
        let count = (input.mixed.len() / FRAME) as f64;
        wall.push(start_wall.elapsed().as_secs_f64() * 1e6 / count);
        cpu.push(start_cpu.elapsed().as_secs_f64() * 1e6 / count);
    }
    cpu.sort_by(f64::total_cmp);
    wall.sort_by(f64::total_cmp);
    println!("| {name} | {erle:.3} | {latency:.3} | {:.3} | {:.3} | {:.3} | {ns:.3} | {gain:.3} | {:.3} |", cpu[4], cpu[0], cpu[8], rms_db(&output[20*16_000..40*16_000]));
    eprintln!("{name}: wall mean us/frame median={:.3}; range {:.3}..{:.3}; echo-only residual {:.3} dBFS", wall[4], wall[0], wall[8], rms_db(&output[10*16_000..20*16_000]));
    Metrics {
        erle,
        latency,
        cpu: cpu[4],
    }
}
fn main() {
    let input = Inputs::new();
    // Also exercise the actual adapter's reset path before measurements.
    EchoCanceller::new().unwrap().reinitialize();
    println!("| Engine | ERLE dB | Added signal delay ms | CPU us/10ms | CPU min | CPU max | Noise attenuation dB | Near-only gain dB | Double-talk output dBFS |");
    println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|");
    let cpp = measure("C++ 2.1.0", cpp, &input);
    let sonora = measure("Sonora 0.2.0", || EchoCanceller::new().unwrap(), &input);
    let matches = (sonora.erle - cpp.erle).abs() <= 1.0
        && sonora.latency <= cpp.latency * 1.1
        && sonora.cpu <= cpp.cpu * 1.1;
    println!(
        "\n{}: ERLE delta {:+.3} dB; delay {:.3} -> {:.3} ms; CPU {:+.1}%.",
        if matches { "MATCH" } else { "DIFFERS" },
        sonora.erle - cpp.erle,
        cpp.latency,
        sonora.latency,
        100.0 * (sonora.cpu / cpp.cpu - 1.0)
    );
}
