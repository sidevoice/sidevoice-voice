//! The native microphone and speaker without devices: what the output callback plays and reports, and the capture's
//! way through the echo canceller, at a speaker's and a microphone's own rates.

use std::sync::atomic::Ordering;

use rtrb::RingBuffer;

use super::drain;
use super::output::{Counters, Output, Queue, Schedule};
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

#[test]
fn a_frame_the_microphone_has_only_begun_stays_for_the_next_drain() {
    let (mut microphone, mut capture) = RingBuffer::new(16);
    // Two stereo frames and the left sample of a third.
    for sample in [0.1, 0.2, 0.3, 0.4, 0.5] {
        microphone.push(sample).unwrap();
    }
    let mut heard = Vec::new();
    drain(&mut capture, &mut heard, 2);
    assert_eq!(heard, [0.1, 0.2, 0.3, 0.4]);
    microphone.push(0.6).unwrap();
    drain(&mut capture, &mut heard, 2);
    assert_eq!(heard, [0.5, 0.6]);
}

#[test]
fn a_chunk_the_ring_has_no_room_for_waits_whole_and_is_never_played_short() {
    let (mut ring, mut speaker) = RingBuffer::new(100);
    let mut queue = Queue::default();
    queue.play("u".into(), 0, vec![0.5; 80]);
    queue.play("u".into(), 1, vec![0.5; 80]);
    queue.feed(&mut ring);
    // Chunk 0 is in; chunk 1 is in by 20 samples only, so it is not scheduled yet.
    let played = |events: &[IoEvent]| {
        events
            .iter()
            .filter(|event| matches!(event, IoEvent::ChunkPlayed { .. }))
            .count()
    };
    assert_eq!(played(&queue.schedule.due(80)), 1);
    assert!(queue.schedule.due(100).is_empty(), "chunk 1 is not all in");
    // The speaker takes 70 samples: the rest of chunk 1 goes in, and it ends at its own last sample.
    for _ in 0..70 {
        speaker.pop().unwrap();
    }
    queue.feed(&mut ring);
    let events = queue.schedule.due(159);
    assert_eq!(
        events,
        [IoEvent::ChunkStarted {
            utterance: "u".into(),
            chunk: 1
        }]
    );
    assert_eq!(played(&queue.schedule.due(160)), 1);
}

#[test]
fn an_empty_chunk_starts_and_ends_in_its_place() {
    let (mut ring, _speaker) = RingBuffer::new(10);
    let mut queue = Queue::default();
    queue.play("u".into(), 0, Vec::new());
    queue.feed(&mut ring);
    assert_eq!(
        queue.schedule.due(0),
        [
            IoEvent::ChunkStarted {
                utterance: "u".into(),
                chunk: 0
            },
            IoEvent::ChunkPlayed {
                utterance: "u".into(),
                chunk: 0
            },
        ]
    );
}

#[test]
fn a_stop_drops_what_waits_and_flushes_what_is_in_the_ring() {
    let (mut ring, _speaker) = RingBuffer::new(10);
    let mut queue = Queue::default();
    let counters = Counters::default();
    queue.play("u".into(), 0, vec![0.5; 8]);
    queue.play("u".into(), 1, vec![0.5; 8]);
    queue.feed(&mut ring);
    queue.stop(&counters);
    assert_eq!(counters.flush_to.load(Ordering::Acquire), 10);
    assert!(queue.schedule.due(1_000).is_empty());
    queue.feed(&mut ring);
    assert!(
        queue.schedule.due(1_000).is_empty(),
        "nothing was left waiting"
    );
}
