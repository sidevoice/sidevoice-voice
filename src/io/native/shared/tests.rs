//! The physical backend is a fake; routing and mixing run through the public lane I/O and sinks.

use std::sync::{Arc, Mutex};

use futures_channel::mpsc::{unbounded, UnboundedReceiver};
use futures_util::StreamExt;

use super::*;

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
                utterance: name.into(), chunk: 0, samples: vec![0.75; FRAME], cursor: 0,
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
