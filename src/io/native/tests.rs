//! The native microphone and speaker without devices: what the output callback plays and reports, and the capture's
//! way through the echo canceller, at a speaker's and a microphone's own rates.

use std::sync::atomic::Ordering;

use rtrb::RingBuffer;

use super::output::{Counters, Output, Schedule};
use super::pipeline::Pipeline;
use crate::io::IoEvent;
use crate::test_support::{clip, QUILTER};

fn output(fade: usize) -> (rtrb::Producer<f32>, Output, rtrb::Consumer<f32>, Counters) {
    let (playback, queued) = RingBuffer::new(1_000);
    let (reference, played) = RingBuffer::new(1_000);
    (
        playback,
        Output::new(queued, reference, fade),
        played,
        Counters::default(),
    )
}

#[test]
fn what_plays_is_the_reference_and_silence_fills_the_gaps() {
    let (mut playback, mut player, mut reference, counters) = output(4);
    for sample in [0.1, 0.2, 0.3] {
        playback.push(sample).unwrap();
    }
    let out: Vec<f32> = (0..5).map(|_| player.next(&counters)).collect();
    assert_eq!(out, [0.1, 0.2, 0.3, 0.0, 0.0]);
    player.publish(&counters);
    assert_eq!(counters.consumed.load(Ordering::Acquire), 3);
    let copied: Vec<f32> = (0..5).map(|_| reference.pop().unwrap()).collect();
    assert_eq!(copied, out);
}

#[test]
fn a_stop_fades_out_and_drops_only_what_was_queued_before_it() {
    let (mut playback, mut player, _reference, counters) = output(4);
    for _ in 0..100 {
        playback.push(1.0).unwrap();
    }
    counters.produced.store(100, Ordering::Release);
    assert_eq!(player.next(&counters), 1.0);
    counters.flush_to.store(100, Ordering::Release);
    // A chunk queued after the stop.
    for _ in 0..3 {
        playback.push(0.5).unwrap();
    }
    let out: Vec<f32> = (0..9).map(|_| player.next(&counters)).collect();
    assert_eq!(out, [1.0, 0.75, 0.5, 0.25, 0.0, 0.5, 0.5, 0.5, 0.0]);
    player.publish(&counters);
    assert_eq!(counters.consumed.load(Ordering::Acquire), 103);
}

#[test]
fn chunks_start_with_their_first_sample_and_end_with_their_last() {
    let mut schedule = Schedule::default();
    schedule.place("u", 0, 0, 100);
    schedule.place("u", 1, 100, 150);
    assert!(schedule.due(0).is_empty());
    let started = IoEvent::ChunkStarted {
        utterance: "u".into(),
        chunk: 0,
    };
    assert_eq!(schedule.due(1), [started]);
    assert_eq!(
        schedule.due(120),
        [
            IoEvent::ChunkPlayed {
                utterance: "u".into(),
                chunk: 0
            },
            IoEvent::ChunkStarted {
                utterance: "u".into(),
                chunk: 1
            },
        ]
    );
    schedule.clear();
    assert!(schedule.due(1_000).is_empty());
}

/// `samples` at 16 kHz, held at 48 kHz (each sample three times): enough for a speaker's and a microphone's rate.
fn at_48k(samples: &[f32]) -> Vec<f32> {
    samples.iter().flat_map(|&s| [s; 3]).collect()
}

#[test]
fn the_capture_comes_out_at_16_khz_in_frames_without_the_echo() {
    let reply = at_48k(&clip(QUILTER, 0.7));
    let mut pipeline = Pipeline::new(48_000, 2, 48_000).unwrap();
    let mut heard = 0;
    let mut echo_in = 0.0_f64;
    let mut echo_out = 0.0_f64;
    // The speaker plays the reply in 10 ms blocks, and the microphone (stereo) hears it back 60 ms later, at -6 dB.
    let delay = 48 * 60;
    for (index, block) in reply.chunks(480).enumerate() {
        pipeline.render(block);
        let start = (index * 480).saturating_sub(delay);
        let mic: Vec<f32> = reply[start..start + block.len()]
            .iter()
            .flat_map(|&s| [s * 0.5, s * 0.5])
            .collect();
        for frame in pipeline.capture(&mic) {
            heard += frame.len();
            if index * 480 > reply.len() / 2 {
                echo_in += mic.iter().map(|&s| f64::from(s * s)).sum::<f64>() / 6.0;
                echo_out += frame.iter().map(|&s| f64::from(s * s)).sum::<f64>();
            }
        }
    }
    assert!(
        (reply.len() / 3).abs_diff(heard) < 320,
        "{heard} of {}",
        reply.len() / 3
    );
    let removed = 10.0 * (echo_in / echo_out.max(1e-12)).log10();
    assert!(removed > 10.0, "{removed:.1} dB of echo removed");
}
