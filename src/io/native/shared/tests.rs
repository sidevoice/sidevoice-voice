//! The physical backend is a fake (`Physical`), or a stand-in that plays as the native speaker does (`Device`): its
//! own playout and output callback, at irregular callback sizes, starving when the test says so. Routing and mixing
//! run through the public lane I/O and sinks.

use std::sync::{Arc, Mutex};

use futures_channel::mpsc::{unbounded, UnboundedReceiver};
use futures_util::StreamExt;

use super::*;
use crate::io::native::output::{Counters, Output, ReferenceSample};
use crate::io::native::pipeline::Pipeline;
use crate::io::native::playout::{Command, Playout};

#[derive(Default)]
struct Recorded {
    starts: usize,
    stops: usize,
    sink: Option<IoSink>,
    played: Vec<(String, Vec<f32>)>,
}

struct Physical(Arc<Mutex<Recorded>>);

impl AudioIo for Physical {
    fn start(&mut self, sink: IoSink) -> Result<(), String> {
        let mut recorded = self.0.lock().unwrap();
        recorded.starts += 1;
        recorded.sink = Some(sink);
        Ok(())
    }
    fn play(&mut self, utterance: &str, _: usize, samples: Vec<f32>, _: u32) {
        self.0
            .lock()
            .unwrap()
            .played
            .push((utterance.into(), samples));
    }
    fn stop_playback(&mut self) {}
    fn stop(&mut self) {
        self.0.lock().unwrap().stops += 1;
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_time()
        .build()
        .unwrap()
}

fn lane_sink() -> (IoSink, UnboundedReceiver<Message>) {
    let (messages, received) = unbounded();
    (
        IoSink {
            messages,
            generation: 1,
            rendered: Arc::new(Mutex::new(RenderedMailbox::default())),
            direct_rendered: false,
        },
        received,
    )
}

async fn next_io(events: &mut UnboundedReceiver<Message>) -> IoEvent {
    loop {
        if let Message::Io(_, event) = events.next().await.expect("event") {
            return event;
        }
    }
}

async fn next_rendered(
    events: &mut UnboundedReceiver<Message>,
    mailbox: &Arc<Mutex<RenderedMailbox>>,
) -> Vec<f32> {
    loop {
        if let Message::Rendered(generation) = events.next().await.expect("rendered event") {
            return mailbox
                .lock()
                .unwrap()
                .take(generation)
                .expect("rendered frame")
                .samples;
        }
    }
}

#[test]
fn unused_shared_audio_does_not_keep_its_worker_or_devices_alive() {
    runtime().block_on(async {
        let recorded = Arc::new(Mutex::new(Recorded::default()));
        let shared = SharedNativeAudio::with_physical(
            NativeDevices::default(),
            Box::new(Physical(recorded)),
        );
        let weak = Arc::downgrade(&shared.inner);
        drop(shared);
        tokio::task::yield_now().await;
        assert!(weak.upgrade().is_none());
    });
}

#[test]
fn next_frame_goes_to_one_port_and_other_lanes_hear_silence() {
    runtime().block_on(async {
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    let shared = SharedNativeAudio::with_physical(NativeDevices::default(), Box::new(Physical(Arc::clone(&recorded))));
    let (mut a, a_port) = shared.attach();
    let (mut b, b_port) = shared.attach();
    let (a_sink, mut a_events) = lane_sink();
    let (b_sink, mut b_events) = lane_sink();
    a.start(a_sink).unwrap();
    b.start(b_sink).unwrap();
    assert!(matches!(next_io(&mut a_events).await, IoEvent::Ready));
    assert!(matches!(next_io(&mut b_events).await, IoEvent::Ready));
    assert_eq!(recorded.lock().unwrap().starts, 1);
    shared.route_input(Some(&a_port)).await.unwrap();
    recorded.lock().unwrap().sink.as_ref().unwrap().send(IoEvent::Captured(vec![0.6; FRAME]));
    assert!(matches!(next_io(&mut a_events).await, IoEvent::Captured(samples) if samples == vec![0.6; FRAME]));
    assert!(matches!(next_io(&mut b_events).await, IoEvent::Captured(samples) if samples == vec![0.0; FRAME]));
    shared.route_input(Some(&b_port)).await.unwrap();
    recorded.lock().unwrap().sink.as_ref().unwrap().send(IoEvent::Captured(vec![0.7; FRAME]));
    assert!(matches!(next_io(&mut a_events).await, IoEvent::Captured(samples) if samples == vec![0.0; FRAME]));
    assert!(matches!(next_io(&mut b_events).await, IoEvent::Captured(samples) if samples == vec![0.7; FRAME]));
    a.stop();
    assert_eq!(recorded.lock().unwrap().stops, 0);
    recorded.lock().unwrap().sink.as_ref().unwrap().send(IoEvent::Captured(vec![0.8; FRAME]));
    assert!(matches!(next_io(&mut b_events).await, IoEvent::Captured(samples) if samples == vec![0.8; FRAME]));
    b.stop();
    assert_eq!(recorded.lock().unwrap().stops, 1);
    });
}

#[test]
fn two_lanes_mix_once_and_keep_their_own_progress_and_rendered_frames() {
    runtime().block_on(async {
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    let shared = SharedNativeAudio::with_physical(NativeDevices::default(), Box::new(Physical(Arc::clone(&recorded))));
    let (mut a, _) = shared.attach();
    let (mut b, _) = shared.attach();
    let (a_sink, mut a_events) = lane_sink();
    let (b_sink, mut b_events) = lane_sink();
    let a_rendered = Arc::clone(&a_sink.rendered);
    let b_rendered = Arc::clone(&b_sink.rendered);
    a.start(a_sink).unwrap();
    b.start(b_sink).unwrap();
    next_io(&mut a_events).await;
    next_io(&mut b_events).await;
    // Queue both before the worker mixes; this also exercises the exact clipping scale.
    {
        let mut state = shared.inner.state.lock().unwrap();
        for (id, name) in [(1, "one"), (2, "two")] {
            state.lanes.get_mut(&id).unwrap().queued.push_back(Chunk {
                utterance: name.into(), chunk: 0, samples: vec![0.75; FRAME].into(), cursor: 0, announced: false,
            });
        }
        state.prime();
    }
    let (frame, mixed) = recorded.lock().unwrap().played[0].clone();
    assert!(mixed.iter().all(|sample| *sample == 1.0));
    let physical_sink = recorded.lock().unwrap().sink.as_ref().unwrap().clone();
    physical_sink.send(IoEvent::ChunkStarted { utterance: frame.clone(), chunk: 0 });
    assert!(matches!(next_io(&mut a_events).await, IoEvent::ChunkStarted { utterance, .. } if utterance == "one"));
    assert!(matches!(next_io(&mut b_events).await, IoEvent::ChunkStarted { utterance, .. } if utterance == "two"));
    physical_sink.send(IoEvent::Rendered(mixed));
    // Rendered samples arrive through IoSink's mailbox, so each lane gets its own scaled 0.5 contribution.
    for (message, mailbox) in [(a_events.next().await.unwrap(), a_rendered), (b_events.next().await.unwrap(), b_rendered)] {
        if let Message::Rendered(generation) = message {
            let frame = mailbox.lock().unwrap().take(generation).unwrap();
            assert!(frame.samples.iter().all(|sample| *sample == 0.5));
        } else { panic!("expected per-lane rendered samples"); }
    }
    physical_sink.send(IoEvent::ChunkPlayed { utterance: frame, chunk: 0 });
    assert!(matches!(next_io(&mut a_events).await, IoEvent::ChunkPlayed { utterance, .. } if utterance == "one"));
    assert!(matches!(next_io(&mut b_events).await, IoEvent::ChunkPlayed { utterance, .. } if utterance == "two"));
    });
}

#[test]
fn a_delayed_mixed_frame_cannot_enter_a_restarted_lanes_sink() {
    runtime().block_on(async {
        let recorded = Arc::new(Mutex::new(Recorded::default()));
        let shared = SharedNativeAudio::with_physical(NativeDevices::default(), Box::new(Physical(Arc::clone(&recorded))));
        let (mut a, _) = shared.attach();
        let (mut b, _) = shared.attach();
        let (old_sink, mut old_events) = lane_sink();
        let (b_sink, mut b_events) = lane_sink();
        a.start(old_sink).unwrap(); b.start(b_sink).unwrap();
        next_io(&mut old_events).await; next_io(&mut b_events).await;
        {
            let mut state = shared.inner.state.lock().unwrap();
            for (id, utterance) in [(1, "old-a"), (2, "live-b")] {
                state.lanes.get_mut(&id).unwrap().queued.push_back(Chunk {
                    utterance: utterance.into(), chunk: 0, samples: vec![0.4; FRAME].into(), cursor: 0, announced: false,
                });
            }
            state.prime();
        }
        let old = recorded.lock().unwrap().played[0].0.clone();
        a.stop();
        // A's stop cut the shared frame; B's part of it is mixed again, alone.
        let (frame, mixed) = recorded.lock().unwrap().played.last().unwrap().clone();
        assert_ne!(frame, old);
        assert!(mixed.iter().all(|sample| (*sample - 0.4).abs() < 0.001));
        let (new_sink, mut new_events) = lane_sink();
        a.start(new_sink).unwrap();
        assert!(matches!(next_io(&mut new_events).await, IoEvent::Ready));
        let physical = recorded.lock().unwrap().sink.as_ref().unwrap().clone();
        // The cut frame's late reports reach nobody.
        physical.send(IoEvent::ChunkStarted { utterance: old.clone(), chunk: 0 });
        physical.send(IoEvent::ChunkPlayed { utterance: old, chunk: 0 });
        physical.send(IoEvent::ChunkStarted { utterance: frame.clone(), chunk: 0 });
        assert!(matches!(next_io(&mut b_events).await, IoEvent::ChunkStarted { utterance, .. } if utterance == "live-b"));
        physical.send(IoEvent::Rendered(mixed));
        let b_mailbox = shared.inner.state.lock().unwrap().lanes.get(&2).unwrap().sink.as_ref().unwrap().rendered.clone();
        assert!(next_rendered(&mut b_events, &b_mailbox).await.iter().all(|sample| (*sample - 0.4).abs() < 0.001));
        physical.send(IoEvent::ChunkPlayed { utterance: frame, chunk: 0 });
        assert!(matches!(next_io(&mut b_events).await, IoEvent::ChunkPlayed { utterance, .. } if utterance == "live-b"));
        assert!(new_events.try_recv().is_err(), "A's replacement sink receives no old progress or meter");
        a.stop(); b.stop();
    });
}

#[test]
fn rendered_levels_follow_partial_physical_consumption_and_never_anticipate_a_lost_tail() {
    runtime().block_on(async {
        let recorded = Arc::new(Mutex::new(Recorded::default()));
        let shared = SharedNativeAudio::with_physical(
            NativeDevices::default(),
            Box::new(Physical(Arc::clone(&recorded))),
        );
        let (mut lane, _) = shared.attach();
        let (sink, mut events) = lane_sink();
        let mailbox = Arc::clone(&sink.rendered);
        lane.start(sink).unwrap();
        next_io(&mut events).await;
        lane.play(
            "late-peak",
            0,
            [vec![0.0; 120], vec![0.9; 40]].concat(),
            RATE,
        );
        let frame = recorded.lock().unwrap().played[0].0.clone();
        let physical = recorded.lock().unwrap().sink.as_ref().unwrap().clone();
        physical.send(IoEvent::ChunkStarted {
            utterance: frame.clone(),
            chunk: 0,
        });
        assert!(matches!(
            next_io(&mut events).await,
            IoEvent::ChunkStarted { .. }
        ));
        physical.send(IoEvent::Rendered(vec![0.0; 80]));
        assert!(next_rendered(&mut events, &mailbox)
            .await
            .iter()
            .all(|sample| *sample == 0.0));
        physical.send(IoEvent::Rendered(vec![0.0; 40]));
        assert!(next_rendered(&mut events, &mailbox)
            .await
            .iter()
            .all(|sample| *sample == 0.0));
        physical.send(IoEvent::Failed("speaker-lost".into()));
        assert!(
            matches!(next_io(&mut events).await, IoEvent::Failed(code) if code == "speaker-lost")
        );
        assert!(
            events.try_recv().is_err(),
            "a tail that never rendered cannot create a peak"
        );

        let (new_sink, mut restarted) = lane_sink();
        let new_mailbox = Arc::clone(&new_sink.rendered);
        lane.start(new_sink).unwrap();
        next_io(&mut restarted).await;
        lane.play(
            "late-peak-again",
            0,
            [vec![0.0; 120], vec![0.9; 40]].concat(),
            RATE,
        );
        let frame = recorded.lock().unwrap().played.last().unwrap().0.clone();
        let physical = recorded.lock().unwrap().sink.as_ref().unwrap().clone();
        physical.send(IoEvent::ChunkStarted {
            utterance: frame,
            chunk: 0,
        });
        next_io(&mut restarted).await;
        physical.send(IoEvent::Rendered(vec![0.0; 120]));
        assert!(next_rendered(&mut restarted, &new_mailbox)
            .await
            .iter()
            .all(|sample| *sample == 0.0));
        physical.send(IoEvent::Rendered(vec![0.9; 40]));
        let rendered = next_rendered(&mut restarted, &new_mailbox).await;
        assert_eq!(rendered.len(), 40);
        assert!(rendered[..39]
            .iter()
            .all(|sample| (*sample - 0.9).abs() < 0.001));
        assert_eq!(
            rendered[39], 0.0,
            "the resampler has not emitted its final input sample"
        );
        lane.stop();
    });
}

#[test]
fn rendered_before_start_preserves_a_queued_silent_prefix_and_its_peak() {
    runtime().block_on(async {
        let recorded = Arc::new(Mutex::new(Recorded::default()));
        let shared = SharedNativeAudio::with_physical(
            NativeDevices::default(),
            Box::new(Physical(Arc::clone(&recorded))),
        );
        let (mut lane, _) = shared.attach();
        let (sink, mut events) = lane_sink();
        let mailbox = Arc::clone(&sink.rendered);
        lane.start(sink).unwrap();
        next_io(&mut events).await;
        lane.play(
            "silent-prefix",
            0,
            [vec![0.0; 120], vec![0.9; 40]].concat(),
            RATE,
        );
        let frame = recorded.lock().unwrap().played[0].0.clone();
        let physical = recorded.lock().unwrap().sink.as_ref().unwrap().clone();

        // The native worker reports rendered samples before the progress marker. Idle output and
        // the chunk's own leading silence have the same amplitude but different callback markers.
        physical.send_physical_rendered(vec![0.0; 100], [vec![false; 20], vec![true; 80]].concat());
        physical.send(IoEvent::ChunkStarted {
            utterance: frame.clone(),
            chunk: 0,
        });
        assert!(matches!(
            next_io(&mut events).await,
            IoEvent::ChunkStarted { .. }
        ));
        assert_eq!(next_rendered(&mut events, &mailbox).await, vec![0.0; 80]);
        assert_eq!(
            shared.inner.state.lock().unwrap().frames[&frame].rendered,
            80
        );

        physical.send_physical_rendered(vec![0.0; 40], vec![true; 40]);
        assert_eq!(next_rendered(&mut events, &mailbox).await, vec![0.0; 40]);
        physical.send_physical_rendered(vec![0.9; 40], vec![true; 40]);
        let peak = next_rendered(&mut events, &mailbox).await;
        assert_eq!(peak.len(), 40);
        assert!(peak[..39]
            .iter()
            .all(|sample| (*sample - 0.9).abs() < 0.001));
        assert_eq!(peak[39], 0.0);
        assert_eq!(
            shared.inner.state.lock().unwrap().frames[&frame].rendered,
            FRAME
        );
        lane.stop();
    });
}

/// The native speaker without a device: the audio thread's own playout (one resampler to 48 kHz, the playback ring,
/// the meter and the chunk schedule) and the real output callback, which the test runs at the sizes it chooses.
struct Device(Arc<Mutex<DeviceState>>);

struct DeviceState {
    sink: Option<IoSink>,
    commands: VecDeque<Command>,
    playout: Playout,
    playback: rtrb::Producer<f32>,
    output: Output,
    counters: Counters,
    pipeline: Pipeline,
    /// What the callback copies, and the copy the playout drains.
    taken: rtrb::Consumer<ReferenceSample>,
    reference: rtrb::Producer<ReferenceSample>,
    reference_out: rtrb::Consumer<ReferenceSample>,
    /// Everything the speaker sounded.
    heard: Vec<ReferenceSample>,
}

const DEVICE_RATE: u32 = 48_000;

impl Device {
    fn new() -> (Self, Arc<Mutex<DeviceState>>) {
        let (playback, queued) = rtrb::RingBuffer::new(DEVICE_RATE as usize * 4);
        let (copy, taken) = rtrb::RingBuffer::new(DEVICE_RATE as usize * 4);
        let (reference, reference_out) = rtrb::RingBuffer::new(DEVICE_RATE as usize * 4);
        let state = Arc::new(Mutex::new(DeviceState {
            sink: None,
            commands: VecDeque::new(),
            playout: Playout::new(DEVICE_RATE),
            playback,
            output: Output::new(
                queued,
                copy,
                (DEVICE_RATE * crate::io::native::FADE_MS / 1000) as usize,
            ),
            counters: Counters::default(),
            pipeline: Pipeline::new(RATE, 1, DEVICE_RATE).unwrap(),
            taken,
            reference,
            reference_out,
            heard: Vec::new(),
        }));
        (Self(Arc::clone(&state)), state)
    }
}

impl AudioIo for Device {
    fn start(&mut self, sink: IoSink) -> Result<(), String> {
        sink.send(IoEvent::Ready);
        self.0.lock().unwrap().sink = Some(sink);
        Ok(())
    }
    fn play(&mut self, utterance: &str, chunk: usize, samples: Vec<f32>, sample_rate: u32) {
        self.0.lock().unwrap().commands.push_back(Command::Play {
            utterance: utterance.into(),
            chunk,
            samples,
            sample_rate,
        });
    }
    fn stop_playback(&mut self) {
        self.0.lock().unwrap().commands.push_back(Command::Stop);
    }
    fn stop(&mut self) {
        self.0.lock().unwrap().sink = None;
    }
}

impl DeviceState {
    /// One output callback of `size` samples, then one wake of the audio thread.
    fn step(&mut self, size: usize) {
        for _ in 0..size {
            self.output.next(&self.counters);
        }
        self.output.publish(&self.counters);
        while let Ok(sample) = self.taken.pop() {
            self.heard.push(sample);
            self.reference.push(sample).unwrap();
        }
        let Some(sink) = self.sink.clone() else {
            return;
        };
        for command in std::mem::take(&mut self.commands) {
            self.playout.command(command, &self.counters);
        }
        self.playout
            .exchange(&mut self.playback, &mut self.reference_out);
        self.playout.render(&mut self.pipeline, &sink);
        self.playout.report(
            &self.counters,
            std::iter::empty(),
            &mut self.pipeline,
            &sink,
        );
    }
}

/// Irregular callback sizes, as devices deliver them, cycling.
const CALLBACKS: [usize; 7] = [512, 441, 1_024, 128, 480, 960, 37];

fn current_thread() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
}

/// Lets the shared dispatcher take in everything the device reported.
async fn settle() {
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
}

/// The progress events a lane received so far, without captures, meters or readiness.
fn progress(events: &mut UnboundedReceiver<Message>, into: &mut Vec<IoEvent>) {
    while let Ok(message) = events.try_recv() {
        if let Message::Io(_, event) = message {
            if matches!(
                event,
                IoEvent::ChunkStarted { .. } | IoEvent::ChunkPlayed { .. } | IoEvent::PlaybackEnded
            ) {
                into.push(event);
            }
        }
    }
}

fn started(utterance: &str, chunk: usize) -> IoEvent {
    IoEvent::ChunkStarted {
        utterance: utterance.into(),
        chunk,
    }
}

fn played(utterance: &str, chunk: usize) -> IoEvent {
    IoEvent::ChunkPlayed {
        utterance: utterance.into(),
        chunk,
    }
}

fn sine(rate: u32, seconds: f32, frequency: f32, amplitude: f32) -> Vec<f32> {
    (0..(rate as f32 * seconds) as usize)
        .map(|i| amplitude * (std::f32::consts::TAU * frequency * i as f32 / rate as f32).sin())
        .collect()
}

/// The span of `heard` from its first to its last queued sample.
fn sounded(heard: &[ReferenceSample]) -> &[ReferenceSample] {
    let first = heard
        .iter()
        .position(|sample| sample.queued)
        .expect("something played");
    let last = heard.iter().rposition(|sample| sample.queued).unwrap();
    &heard[first..=last]
}

#[test]
fn a_sine_passes_through_mixing_and_the_speakers_resampling_without_seams() {
    current_thread().block_on(async {
        let (device, state) = Device::new();
        let shared = SharedNativeAudio::with_physical(NativeDevices::default(), Box::new(device));
        let (mut lane, _) = shared.attach();
        let (sink, mut events) = lane_sink();
        lane.start(sink).unwrap();
        // One second at 24 kHz, in chunks of uneven length, as a synthesizer hands them over.
        let (frequency, amplitude) = (440.0, 0.5);
        let speech = sine(24_000, 1.0, frequency, amplitude);
        let mut at = 0;
        for (chunk, length) in [4_800, 7_001, 3_333, 8_866].into_iter().enumerate() {
            lane.play("sine", chunk, speech[at..at + length].to_vec(), 24_000);
            at += length;
        }
        let mut seen = Vec::new();
        for size in CALLBACKS.iter().cycle().take(400) {
            state.lock().unwrap().step(*size);
            settle().await;
            progress(&mut events, &mut seen);
        }
        let expected: Vec<IoEvent> = (0..4)
            .flat_map(|chunk| [started("sine", chunk), played("sine", chunk)])
            .chain([IoEvent::PlaybackEnded])
            .collect();
        assert_eq!(seen, expected);

        let heard = state.lock().unwrap().heard.clone();
        let span = sounded(&heard);
        assert!(
            span.iter().all(|sample| sample.queued),
            "the speaker never ran dry"
        );
        let length = span.len() as i64 - i64::from(DEVICE_RATE);
        assert!(
            length.abs() <= 6,
            "a second in is a second out ({length} samples off)"
        );
        // Away from the edges (the low-pass settling), every step is a sine's step: no seam at any frame boundary.
        let edge = DEVICE_RATE as usize / 50;
        let middle: Vec<f32> = span[edge..span.len() - edge]
            .iter()
            .map(|sample| sample.value)
            .collect();
        let slope = std::f32::consts::TAU * frequency / DEVICE_RATE as f32 * amplitude;
        let jump = middle
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, f32::max);
        assert!(
            jump <= slope * 1.2,
            "largest step {jump}, a sine's is {slope}"
        );
        // And it is the sine, at its frequency: the best-fitting 440 Hz sine leaves almost nothing.
        let omega = std::f64::consts::TAU * f64::from(frequency) / f64::from(DEVICE_RATE);
        let (mut sin, mut cos) = (0.0, 0.0);
        for (i, &value) in middle.iter().enumerate() {
            sin += f64::from(value) * (omega * i as f64).sin();
            cos += f64::from(value) * (omega * i as f64).cos();
        }
        let (sin, cos) = (
            2.0 * sin / middle.len() as f64,
            2.0 * cos / middle.len() as f64,
        );
        let residual = (middle
            .iter()
            .enumerate()
            .map(|(i, &value)| {
                let fit = sin * (omega * i as f64).sin() + cos * (omega * i as f64).cos();
                (f64::from(value) - fit).powi(2)
            })
            .sum::<f64>()
            / middle.len() as f64)
            .sqrt();
        assert!(
            residual < 0.01,
            "residual {residual} against the fitted sine"
        );
        lane.stop();
    });
}

#[test]
fn a_starved_speaker_neither_ends_the_playback_nor_loses_a_chunks_reports() {
    current_thread().block_on(async {
        let (device, state) = Device::new();
        let shared = SharedNativeAudio::with_physical(NativeDevices::default(), Box::new(device));
        let (mut lane, _) = shared.attach();
        let (sink, mut events) = lane_sink();
        lane.start(sink).unwrap();
        for chunk in 0..3 {
            lane.play("long", chunk, sine(RATE, 0.3, 300.0, 0.4), RATE);
        }
        let mut seen = Vec::new();
        for (step, size) in CALLBACKS.iter().cycle().take(600).enumerate() {
            // Now and then the dispatcher falls behind while the device plays on, far past the lead.
            {
                let mut device = state.lock().unwrap();
                device.step(*size);
                if step % 25 == 10 {
                    for _ in 0..6 {
                        device.step(1_024);
                    }
                }
            }
            settle().await;
            progress(&mut events, &mut seen);
        }
        let heard = state.lock().unwrap().heard.clone();
        let span = sounded(&heard);
        assert!(
            span.iter().any(|sample| !sample.queued),
            "the scenario starves the speaker"
        );
        let queued = span.iter().filter(|sample| sample.queued).count() as i64;
        assert!(
            (queued - i64::from(DEVICE_RATE) * 9 / 10).abs() <= 6,
            "every sample given was played ({queued})"
        );
        let expected: Vec<IoEvent> = (0..3)
            .flat_map(|chunk| [started("long", chunk), played("long", chunk)])
            .chain([IoEvent::PlaybackEnded])
            .collect();
        assert_eq!(seen, expected);
        lane.stop();
    });
}

#[test]
fn an_interruption_cuts_the_speaker_within_its_fade_despite_the_lead() {
    current_thread().block_on(async {
        let (device, state) = Device::new();
        let shared = SharedNativeAudio::with_physical(NativeDevices::default(), Box::new(device));
        let (mut lane, _) = shared.attach();
        let (sink, mut events) = lane_sink();
        lane.start(sink).unwrap();
        lane.play("cut", 0, sine(RATE, 2.0, 300.0, 0.4), RATE);
        let mut seen = Vec::new();
        for _ in 0..30 {
            state.lock().unwrap().step(480);
            settle().await;
        }
        progress(&mut events, &mut seen);
        assert_eq!(seen, [started("cut", 0)]);
        let at = state.lock().unwrap().heard.len();
        lane.stop_playback();
        settle().await;
        progress(&mut events, &mut seen);
        assert_eq!(seen, [started("cut", 0), IoEvent::PlaybackEnded]);
        for size in CALLBACKS.iter().cycle().take(40) {
            state.lock().unwrap().step(*size);
            settle().await;
        }
        // One callback still plays before the audio thread wakes to the stop, then the fade: about 40 ms in all.
        let budget = CALLBACKS[0] + (DEVICE_RATE * crate::io::native::FADE_MS / 1000) as usize;
        let heard = state.lock().unwrap().heard.clone();
        assert!(heard[at..at + 480].iter().all(|sample| sample.queued));
        let after = &heard[at + budget..];
        assert!(after.len() > 4_000);
        assert!(
            after
                .iter()
                .all(|sample| sample.value == 0.0 && !sample.queued),
            "silent once the fade is over"
        );
        progress(&mut events, &mut seen);
        assert_eq!(seen.len(), 2, "nothing more is reported for what was cut");
        lane.stop();
    });
}

#[test]
fn a_lane_that_is_interrupted_leaves_every_report_of_the_other_lane() {
    current_thread().block_on(async {
        let (device, state) = Device::new();
        let shared = SharedNativeAudio::with_physical(NativeDevices::default(), Box::new(device));
        let (mut a, _) = shared.attach();
        let (mut b, _) = shared.attach();
        let (a_sink, mut a_events) = lane_sink();
        let (b_sink, mut b_events) = lane_sink();
        a.start(a_sink).unwrap();
        b.start(b_sink).unwrap();
        a.play("a", 0, sine(RATE, 1.0, 300.0, 0.3), RATE);
        for chunk in 0..3 {
            b.play("b", chunk, sine(RATE, 0.2, 500.0, 0.3), RATE);
        }
        let (mut a_seen, mut b_seen) = (Vec::new(), Vec::new());
        for (step, size) in CALLBACKS.iter().cycle().take(300).enumerate() {
            if step == 20 {
                a.stop_playback();
            }
            state.lock().unwrap().step(*size);
            settle().await;
            progress(&mut a_events, &mut a_seen);
            progress(&mut b_events, &mut b_seen);
        }
        assert_eq!(a_seen, [started("a", 0), IoEvent::PlaybackEnded]);
        let expected: Vec<IoEvent> = (0..3)
            .flat_map(|chunk| [started("b", chunk), played("b", chunk)])
            .chain([IoEvent::PlaybackEnded])
            .collect();
        assert_eq!(b_seen, expected);
        a.stop();
        b.stop();
    });
}
