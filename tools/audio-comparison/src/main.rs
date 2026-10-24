//! Reproducible comparison against the former bundled C++ pipeline. See ../README.md.
mod engine;
mod input;
mod audio {
    pub const FRAME: usize = 160;
}
mod turns {
    pub const RATE: u32 = 16_000;
}
#[rustfmt::skip] // Formatted/tested with the product crate.
#[path = "../../../src/echo/native.rs"]
mod native;

use audio::FRAME;
use audio_comparison::{
    db, energy, group_delay, lag, quality, quantile, white_noise, Quality, RATE,
};
use cpu_time::ThreadTime;
use engine::{Engine, Kind};
use input::Inputs;
use std::{hint::black_box, time::Instant};

fn process(mut engine: Box<dyn Engine>, render: &[f32], mic: &[f32]) -> Vec<f32> {
    assert_eq!(render.len(), mic.len());
    let mut out = Vec::with_capacity(mic.len());
    for (render, capture) in render
        .as_chunks::<FRAME>()
        .0
        .iter()
        .zip(mic.as_chunks::<FRAME>().0)
    {
        let mut frame = *capture;
        engine.frame(render, &mut frame);
        assert!(frame.iter().all(|v| v.is_finite()));
        out.extend(frame);
    }
    out
}
struct Acoustics {
    erle: f64,
    buffering: usize,
    hpf_delay: [f64; 4],
    near: Quality,
    double: Quality,
    near_clean: Quality,
    double_clean: Quality,
    noise_attenuation: f64,
}
fn acoustic(kind: Kind, input: &Inputs) -> Acoustics {
    // Match the full pipeline against its own isolated HPF on a broadband probe.
    // HPF phase is present in both paths and is not mistaken for buffering.
    let probe = white_noise(16 * RATE, 0.1);
    let zero = vec![0.0; probe.len()];
    let reference = process(kind.hpf(), &zero, &probe);
    let output = process(kind.full(), &zero, &probe);
    let lags: Vec<_> = [10, 12, 14]
        .into_iter()
        .map(|sec| lag(&reference, &output, sec * RATE, RATE))
        .collect();
    println!(
        "{} phase-matched broadband lag/correlation (10/12/14 s): {:?}",
        kind.name(),
        lags
    );
    let buffering = lags[0].0;
    assert!(
        lags.iter()
            .all(|&(lag, corr)| lag == buffering && corr > 0.8),
        "unstable/uncorrelated buffering probe"
    );
    assert!(buffering < 320, "buffering search hit its boundary");

    let mut impulse = vec![0.0; RATE];
    impulse[0] = 0.125;
    let hpf_ir = process(kind.hpf(), &vec![0.0; RATE], &impulse);
    let hpf_delay = [100.0, 300.0, 1000.0, 3000.0].map(|hz| group_delay(&hpf_ir, hz));
    let output = process(kind.full(), &input.render, &input.mixed);
    let erle =
        db(energy(&input.echo[10 * RATE..20 * RATE]) / energy(&output[10 * RATE..20 * RATE]));
    let zero = vec![0.0; input.near.len()];
    let near = process(kind.full(), &zero, &input.near);
    let filtered_near = process(kind.hpf(), &zero, &input.near);
    // Preserve the actual near-end onset at 20 s in the double-talk reference.
    let gated: Vec<f32> = input
        .near
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            if (20 * RATE..40 * RATE).contains(&i) {
                v
            } else {
                0.0
            }
        })
        .collect();
    let filtered_double = process(kind.hpf(), &zero, &gated);
    let score = |reference: &[f32], output: &[f32], start: usize, end: usize| {
        quality(
            &reference[start * RATE..end * RATE],
            &output[start * RATE + buffering..end * RATE + buffering],
        )
    };
    let noise = process(kind.full(), &zero, &input.noise);
    Acoustics {
        erle,
        buffering,
        hpf_delay,
        near: score(&filtered_near, &near, 10, 20),
        double: score(&filtered_double, &output, 20, 40),
        near_clean: score(&input.near, &near, 10, 20),
        double_clean: score(&gated, &output, 20, 40),
        noise_attenuation: db(energy(&input.noise[10 * RATE..]) / energy(&noise[10 * RATE..])),
    }
}

fn timed(kind: Kind, input: &Inputs) -> (f64, f64) {
    let mut engine = kind.full();
    let render = input.render.as_chunks::<FRAME>().0;
    let mic = input.mixed.as_chunks::<FRAME>().0;
    // Process seconds 0–10 with this same instance before either timer starts.
    let warmup = 10 * RATE / FRAME;
    for (r, c) in render[..warmup].iter().zip(&mic[..warmup]) {
        let mut frame = *c;
        engine.frame(r, &mut frame);
        black_box(frame);
    }
    let cpu = ThreadTime::now();
    let wall = Instant::now();
    for (r, c) in render[warmup..].iter().zip(&mic[warmup..]) {
        let mut frame = *c;
        engine.frame(black_box(r), black_box(&mut frame));
        black_box(frame);
    }
    let elapsed_wall = wall.elapsed().as_secs_f64();
    let elapsed_cpu = cpu.elapsed().as_secs_f64();
    let frames = (mic.len() - warmup) as f64;
    (elapsed_cpu * 1e6 / frames, elapsed_wall * 1e6 / frames)
}
fn row(label: &str, a: f64, b: f64) {
    println!("| {label} | {a:.3} | {b:.3} |");
}
fn main() {
    let input = Inputs::new();
    native::EchoCanceller::new().unwrap().reinitialize();
    let cpp = acoustic(Kind::Cpp, &input);
    let sonora = acoustic(Kind::Sonora, &input);
    let mut cpu: [Vec<f64>; 2] = std::array::from_fn(|_| Vec::new());
    let mut wall: [Vec<f64>; 2] = std::array::from_fn(|_| Vec::new());
    println!("\n| Iteration | First engine | C++ CPU us/frame | Sonora CPU us/frame |\n|---|---|---:|---:|");
    for iteration in 0..12 {
        let order = if iteration % 2 == 0 {
            [Kind::Cpp, Kind::Sonora]
        } else {
            [Kind::Sonora, Kind::Cpp]
        };
        for kind in order {
            let index = match kind {
                Kind::Cpp => 0,
                Kind::Sonora => 1,
            };
            let (c, w) = timed(kind, &input);
            cpu[index].push(c);
            wall[index].push(w);
        }
        println!(
            "| {} | {} | {:.3} | {:.3} |",
            iteration + 1,
            order[0].name(),
            cpu[0][iteration],
            cpu[1][iteration]
        );
    }
    for values in cpu.iter_mut().chain(wall.iter_mut()) {
        values.sort_by(f64::total_cmp);
    }
    let cpu_median = cpu.each_ref().map(|v| quantile(v, 0.5));
    println!("\n| Metric | C++ 2.1.0 | Sonora 0.2.0 |\n|---|---:|---:|");
    row("Full-pipeline ERLE dB", cpp.erle, sonora.erle);
    row(
        "Buffering delay ms (HPF phase matched)",
        cpp.buffering as f64 / 16.0,
        sonora.buffering as f64 / 16.0,
    );
    for (i, hz) in [100, 300, 1000, 3000].iter().enumerate() {
        row(
            &format!("HPF group delay ms at {hz} Hz"),
            cpp.hpf_delay[i],
            sonora.hpf_delay[i],
        );
    }
    row(
        "Near-only SI-SDR dB (HPF reference)",
        cpp.near.si_sdr,
        sonora.near.si_sdr,
    );
    row(
        "Double-talk SI-SDR dB (HPF reference)",
        cpp.double.si_sdr,
        sonora.double.si_sdr,
    );
    row(
        "Near-only SNR dB (HPF reference, no gain fit)",
        cpp.near.snr,
        sonora.near.snr,
    );
    row(
        "Double-talk SNR dB (HPF reference, no gain fit)",
        cpp.double.snr,
        sonora.double.snr,
    );
    row(
        "Near-only projected gain dB",
        cpp.near.gain,
        sonora.near.gain,
    );
    row(
        "Double-talk projected gain dB",
        cpp.double.gain,
        sonora.double.gain,
    );
    row(
        "Near-only SI-SDR dB (unfiltered reference)",
        cpp.near_clean.si_sdr,
        sonora.near_clean.si_sdr,
    );
    row(
        "Double-talk SI-SDR dB (unfiltered reference)",
        cpp.double_clean.si_sdr,
        sonora.double_clean.si_sdr,
    );
    row(
        "Noise-only attenuation dB",
        cpp.noise_attenuation,
        sonora.noise_attenuation,
    );
    for (label, p) in [
        ("CPU min us/10ms", 0.0),
        ("CPU Q1 us/10ms", 0.25),
        ("CPU median us/10ms", 0.5),
        ("CPU Q3 us/10ms", 0.75),
        ("CPU max us/10ms", 1.0),
    ] {
        row(label, quantile(&cpu[0], p), quantile(&cpu[1], p));
    }
    row(
        "Wall median us/10ms",
        quantile(&wall[0], 0.5),
        quantile(&wall[1], 0.5),
    );
    let overlap = cpu[0][0].max(cpu[1][0]) <= cpu[0][11].min(cpu[1][11]);
    println!(
        "\nCPU min/max ranges overlap: **{}**.",
        if overlap { "yes" } else { "no" }
    );
    let matches = (sonora.erle - cpp.erle).abs() <= 1.0
        && sonora.buffering as f64 <= cpp.buffering as f64 * 1.1
        && cpu_median[1] <= cpu_median[0] * 1.1;
    println!("\n**{}**: ERLE delta {:+.3} dB; buffering {:+.1}%; CPU {:+.1}%. HPF group delay is frequency dependent and reported separately.",
        if matches {"MATCH"} else {"DIFFERS"},sonora.erle-cpp.erle,
        100.0*(sonora.buffering as f64/cpp.buffering as f64-1.0),100.0*(cpu_median[1]/cpu_median[0]-1.0));
}
