//! The native microphone and speaker without devices: what the output callback plays and reports, and the capture's
//! way through the echo canceller, at a speaker's and a microphone's own rates.

use std::sync::atomic::Ordering;

use rtrb::RingBuffer;

use super::drain;
use super::output::{Counters, Output, Queue, ReferenceSample, Schedule};
use super::pipeline::Pipeline;
use super::playout::{playback_complete, send_capture_after_playback};
use crate::call::{Call, Effect, Input};
use crate::event::{TurnEvent, VoiceEvent};
use crate::io::IoEvent;
use crate::say::SayEvent;
use crate::test_support::{clip, config, EnergyVad, FLEURS_ES, QUILTER, WINDOW};

fn output(
    fade: usize,
) -> (
    rtrb::Producer<f32>,
    Output,
    rtrb::Consumer<ReferenceSample>,
    Counters,
) {
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
    let copied: Vec<ReferenceSample> = (0..5).map(|_| reference.pop().unwrap()).collect();
    assert_eq!(
        copied.iter().map(|sample| sample.value).collect::<Vec<_>>(),
        out
    );
    assert_eq!(
        copied
            .iter()
            .map(|sample| sample.queued)
            .collect::<Vec<_>>(),
        [true, true, true, false, false]
    );
}

#[test]
fn completion_waits_for_a_callback_that_publishes_after_the_reference_drain() {
    let (mut playback, mut player, mut reference, counters) = output(4);
    let mut queue = Queue::default();
    queue.play("u".into(), 0, vec![0.8; 160]);
    queue.feed(&mut playback);
    let mut drained = Vec::new();
    drain(&mut reference, &mut drained, 1);
    let mut count = drained.len() as u64;
    // The output callback completes between the worker's drain and its completion observation.
    for _ in 0..160 {
        player.next(&counters);
    }
    player.publish(&counters);
    let consumed = counters.consumed.load(Ordering::Acquire);
    queue.schedule.due(consumed);
    assert!(queue.is_idle());
    assert!(!playback_complete(
        None,
        true,
        true,
        consumed,
        counters.referenced.load(Ordering::Acquire),
        count,
    ));
    drain(&mut reference, &mut drained, 1);
    count += drained.len() as u64;
    assert!(drained.iter().any(|sample| sample.value > 0.0));
    let mut pipeline = Pipeline::new(16_000, 1, 16_000).unwrap();
    let mut events = vec![IoEvent::Rendered(
        pipeline.render(
            &drained
                .iter()
                .map(|sample| sample.value)
                .collect::<Vec<_>>(),
        ),
    )];
    assert!(playback_complete(
        None,
        true,
        true,
        consumed,
        counters.referenced.load(Ordering::Acquire),
        count,
    ));
    events.push(IoEvent::PlaybackEnded);
    pipeline.reset_playback_meter();
    for _ in 0..160 {
        player.next(&counters);
    }
    drain(&mut reference, &mut drained, 1);
    assert!(pipeline
        .render(
            &drained
                .iter()
                .map(|sample| sample.value)
                .collect::<Vec<_>>()
        )
        .iter()
        .all(|sample| *sample == 0.0));
    assert!(matches!(events.last(), Some(IoEvent::PlaybackEnded)));
    assert!(!playback_complete(
        None, false, true, consumed, count, count
    ));
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

#[test]
fn rendered_boundaries_precede_qualifying_capture_even_when_the_driver_is_delayed() {
    let text = "A sufficiently long first sentence. A sufficiently long second sentence.";
    let heard = crate::speech::chunks(text)[0].chars.end;
    let mut schedule = Schedule::default();
    schedule.place("u", 0, 0, 64 * WINDOW as u64);
    schedule.place("u", 1, 64 * WINDOW as u64, 65 * WINDOW as u64);
    let tone: Vec<f32> = (0..WINDOW)
        .map(|sample| 0.3 * (2.0 * std::f32::consts::PI * 1_000.0 * sample as f32 / 16_000.0).sin())
        .collect();
    // The audio worker gets ahead of a busy control task. On the final tick chunk 0 completes,
    // chunk 1 begins, and the capture supplies the last qualifying speech window.
    let mut queued = Vec::new();
    for tick in 1..=64 {
        let consumed = if tick == 64 {
            64 * WINDOW as u64 + 1
        } else {
            tick * WINDOW as u64
        };
        send_capture_after_playback(&mut schedule, consumed, [tone.clone()], |event| {
            queued.push(event)
        });
    }
    assert!(matches!(
        queued.first(),
        Some(IoEvent::ChunkStarted { chunk: 0, .. })
    ));
    assert!(matches!(
        &queued[queued.len() - 3..],
        [
            IoEvent::ChunkPlayed { chunk: 0, .. },
            IoEvent::ChunkStarted { chunk: 1, .. },
            IoEvent::Captured(_)
        ]
    ));

    let mut call = Call::new(config(), "c".into(), 0);
    let mut effects = call.handle(0, Input::Start { take: false });
    effects.extend(call.handle(
        0,
        Input::Say {
            id: "u".into(),
            text: text.into(),
            language: None,
        },
    ));
    for chunk in 0..2 {
        let samples = if chunk == 0 { 64 * WINDOW } else { WINDOW };
        effects.extend(call.handle(
            0,
            Input::Synthesized {
                utterance: "u".into(),
                chunk,
                result: Ok((vec![0.1; samples], 16_000)),
            },
        ));
    }
    let mut captured = 0;
    for event in queued {
        match event {
            IoEvent::ChunkStarted { utterance, chunk } => {
                effects.extend(call.handle(captured * 32, Input::ChunkStarted { utterance, chunk }))
            }
            IoEvent::ChunkPlayed { utterance, chunk } => {
                effects.extend(call.handle(captured * 32, Input::ChunkPlayed { utterance, chunk }))
            }
            IoEvent::Captured(pcm) => {
                captured += 1;
                effects.extend(call.handle(
                    captured * 32,
                    Input::Window {
                        pcm,
                        speech: captured > 32,
                        probability: Some(if captured > 32 { 0.9 } else { 0.02 }),
                    },
                ));
            }
            _ => unreachable!(),
        }
    }
    assert_eq!(captured, 64);
    assert_eq!(
        effects
            .iter()
            .filter(|effect| matches!(
                effect,
                Effect::Event(VoiceEvent::Turn(TurnEvent::Started { .. }))
            ))
            .count(),
        1
    );
    assert_eq!(
        effects
            .iter()
            .filter(|effect| matches!(effect, Effect::StopPlayback))
            .count(),
        1
    );
    assert!(effects.iter().any(|effect| matches!(effect, Effect::Say { event: SayEvent::Done { outcome: crate::say::SayOutcome::HeardUpTo { heard_chars, reason: crate::say::StopReason::BargeIn } }, .. } if *heard_chars == heard)));
}

#[test]
fn recorded_speaker_echo_cannot_start_a_turn_but_a_person_over_it_can() {
    fn scenario(person_peak: Option<f32>, person_ms: usize) -> Vec<Effect> {
        let reply = clip(QUILTER, 0.7);
        // A continuous voiced half-second from the recorded FLEURS person, repeated over the reply.
        // Scale this slice itself: its peak is only about a quarter of the full recording's peak.
        let person_clip = clip(FLEURS_ES, 1.0);
        let slice = &person_clip[144 * WINDOW..160 * WINDOW];
        let slice_peak = slice
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        let voiced: Vec<f32> = slice
            .iter()
            .map(|sample| sample * person_peak.unwrap_or(0.4) / slice_peak)
            .collect();
        let mut call = Call::new(config(), "c".into(), 0);
        let mut effects = call.handle(0, Input::Start { take: false });
        effects.extend(call.handle(
            0,
            Input::Say {
                id: "reply".into(),
                text: "A recorded reply playing on the local speaker.".into(),
                language: None,
            },
        ));
        effects.extend(call.handle(
            0,
            Input::Synthesized {
                utterance: "reply".into(),
                chunk: 0,
                result: Ok((vec![0.1; reply.len()], 16_000)),
            },
        ));
        effects.extend(call.handle(
            0,
            Input::ChunkStarted {
                utterance: "reply".into(),
                chunk: 0,
            },
        ));
        let mut pipeline = Pipeline::new(16_000, 1, 16_000).unwrap();
        let mut vad = EnergyVad::new();
        let mut clean = Vec::new();
        let mut now = 0;
        let end = reply.len() + 2_000 * 16;
        for at in (0..end).step_by(160) {
            let render: Vec<f32> = (at..at + 160)
                .map(|i| reply.get(i).copied().unwrap_or(0.0))
                .collect();
            pipeline.render(&render);
            let mic: Vec<f32> = (at..at + 160)
                .map(|i| {
                    let echo = [(60 * 16, 0.5), (75 * 16, 0.2), (95 * 16, 0.1)]
                        .into_iter()
                        .map(|(delay, gain)| {
                            i.checked_sub(delay)
                                .and_then(|j| reply.get(j))
                                .copied()
                                .unwrap_or(0.0)
                                * gain
                        })
                        .sum::<f32>();
                    let voice = if person_peak.is_some()
                        && (16_000..16_000 + person_ms * 16).contains(&i)
                    {
                        voiced[(i - 16_000) % voiced.len()]
                    } else {
                        0.0
                    };
                    echo + voice
                })
                .collect();
            for frame in pipeline.capture(&mic) {
                clean.extend(frame);
            }
            while clean.len() >= WINDOW {
                let pcm: Vec<f32> = clean.drain(..WINDOW).collect();
                now += 32;
                effects.extend(call.handle(
                    now,
                    Input::Window {
                        speech: vad.window(&pcm),
                        probability: Some(EnergyVad::probability(&pcm)),
                        pcm,
                    },
                ));
            }
            if at < reply.len() && at + 160 >= reply.len() {
                effects.extend(call.handle(
                    now,
                    Input::ChunkPlayed {
                        utterance: "reply".into(),
                        chunk: 0,
                    },
                ));
            }
        }
        effects
    }
    let alone = scenario(None, 0);
    assert!(!alone.iter().any(|effect| matches!(
        effect,
        Effect::Event(VoiceEvent::Turn(_)) | Effect::Transcribe { .. } | Effect::StopPlayback
    )));
    let backchannel = scenario(Some(0.4), 400);
    assert!(!backchannel.iter().any(|effect| matches!(
        effect,
        Effect::Event(VoiceEvent::Turn(_)) | Effect::Transcribe { .. } | Effect::StopPlayback
    )));
    let person = scenario(Some(0.4), 4_000);
    assert!(person.iter().any(|effect| matches!(
        effect,
        Effect::Event(VoiceEvent::Turn(TurnEvent::Started { .. }))
    )));
    assert!(person
        .iter()
        .any(|effect| matches!(effect, Effect::StopPlayback)));
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
        let rendered = pipeline.render(block);
        assert!(!rendered.is_empty());
        assert!(rendered.iter().all(|sample| sample.is_finite()));
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

#[test]
fn a_queue_is_idle_once_nothing_waits_or_is_left_to_play() {
    let (mut playback, _player, _reference, counters) = output(4);
    let mut queue = Queue::default();
    assert!(queue.is_idle());
    queue.play("u".into(), 0, vec![0.5; 10]);
    assert!(!queue.is_idle());
    queue.feed(&mut playback);
    assert!(!queue.is_idle());
    queue.schedule.due(10);
    assert!(queue.is_idle());
    queue.play("u".into(), 1, vec![0.5; 10]);
    queue.stop(&counters);
    assert!(queue.is_idle());
}

#[test]
fn an_abandoned_queue_ends_every_chunk_in_order() {
    let (mut playback, _player, _reference, _counters) = output(4);
    let mut queue = Queue::default();
    queue.play("u".into(), 0, vec![0.5; 10]);
    queue.feed(&mut playback);
    queue.schedule.due(1);
    queue.play("u".into(), 1, vec![0.5; 10]);
    let chunk = |chunk| (String::from("u"), chunk);
    let events: Vec<_> = queue
        .abandon()
        .into_iter()
        .map(|event| match event {
            IoEvent::ChunkStarted { utterance, chunk } => ("started", (utterance, chunk)),
            IoEvent::ChunkPlayed { utterance, chunk } => ("played", (utterance, chunk)),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        events,
        [
            ("played", chunk(0)),
            ("started", chunk(1)),
            ("played", chunk(1)),
        ]
    );
    assert!(queue.is_idle());
}
