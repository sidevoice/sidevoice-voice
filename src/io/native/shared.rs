//! Several call-local audio lanes over one microphone, speaker and echo canceller. The physical speaker receives
//! short mixed frames; its actual playback clock drives each lane's chunk and rendered reports.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};

use futures_channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures_channel::oneshot;
use futures_util::StreamExt;

use super::{NativeDevices, NativeIo};
use crate::audio::Resampler;
use crate::io::{AudioIo, IoEvent, IoSink, RenderedMailbox};
use crate::runtime::spawn;
use crate::voice_call::Message;

const RATE: u32 = 16_000;
const FRAME: usize = 160;
const MIX_AHEAD: usize = 2;

#[cfg(test)]
mod tests;

/// A physical audio pair shared by several independent calls.
#[derive(Clone)]
pub struct SharedNativeAudio {
    inner: Arc<Inner>,
}

/// An opaque input lane belonging to one shared audio pair.
#[derive(Clone)]
pub struct InputPort {
    inner: Arc<Inner>,
    id: u64,
}

struct Inner {
    state: Mutex<State>,
    messages: UnboundedSender<Message>,
    rendered: Arc<Mutex<RenderedMailbox>>,
    devices: NativeDevices,
}

struct State {
    physical: Box<dyn AudioIo>,
    running: bool,
    generation: u64,
    next_lane: u64,
    next_frame: u64,
    routed: Option<u64>,
    lanes: BTreeMap<u64, Lane>,
    frames: HashMap<String, MixedFrame>,
    frame_order: VecDeque<String>,
    rendered_pending: VecDeque<f32>,
}

struct Lane {
    sink: Option<IoSink>,
    queued: VecDeque<Chunk>,
    stopping: bool,
    incarnation: u64,
}

struct Chunk {
    utterance: String,
    chunk: usize,
    samples: Vec<f32>,
    cursor: usize,
}

struct Part {
    lane: u64,
    incarnation: u64,
    utterance: String,
    chunk: usize,
    samples: Vec<f32>,
    first: bool,
    last: bool,
}

struct MixedFrame {
    parts: Vec<Part>,
    mixed: Vec<f32>,
    rendered: usize,
    started: bool,
    played: bool,
}

struct LaneIo {
    inner: Arc<Inner>,
    id: u64,
}

impl SharedNativeAudio {
    /// Opens devices when the first attached lane starts; later lanes reuse them.
    #[must_use]
    pub fn new(devices: NativeDevices) -> Self {
        Self::with_physical(devices.clone(), Box::new(NativeIo::with_devices(devices)))
    }

    fn with_physical(devices: NativeDevices, physical: Box<dyn AudioIo>) -> Self {
        let (messages, received) = unbounded();
        let rendered = Arc::new(Mutex::new(RenderedMailbox::default()));
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                physical,
                running: false,
                generation: 0,
                next_lane: 1,
                next_frame: 0,
                routed: None,
                lanes: BTreeMap::new(),
                frames: HashMap::new(),
                frame_order: VecDeque::new(),
                rendered_pending: VecDeque::new(),
            }),
            messages,
            rendered,
            devices,
        });
        spawn(dispatch(Arc::downgrade(&inner), received));
        Self { inner }
    }

    /// The same device choices every attached lane uses.
    #[must_use]
    pub fn devices(&self) -> NativeDevices {
        self.inner.devices.clone()
    }

    /// An audio lane for a call and the port that can receive actual microphone frames.
    #[must_use]
    pub fn attach(&self) -> (Box<dyn AudioIo>, InputPort) {
        let mut state = self.inner.state.lock().expect("shared audio");
        let id = state.next_lane;
        state.next_lane += 1;
        state.lanes.insert(
            id,
            Lane {
                sink: None,
                queued: VecDeque::new(),
                stopping: false,
                incarnation: 0,
            },
        );
        (
            Box::new(LaneIo {
                inner: Arc::clone(&self.inner),
                id,
            }),
            InputPort {
                inner: Arc::clone(&self.inner),
                id,
            },
        )
    }

    /// Applies a route after all already-reported physical frames. Every started lane continues receiving frames;
    /// only the selected lane receives the real samples. `None` gives every lane silence.
    pub async fn route_input(&self, target: Option<&InputPort>) -> Result<(), String> {
        let id = match target {
            None => 0,
            Some(port) if Arc::ptr_eq(&self.inner, &port.inner) => port.id,
            Some(_) => return Err("audio-input-foreign".into()),
        };
        let (answer, received) = oneshot::channel();
        self.inner
            .messages
            .unbounded_send(Message::SharedRoute(id, answer))
            .map_err(|_| "audio-input-unavailable".to_owned())?;
        received
            .await
            .map_err(|_| "audio-input-unavailable".to_owned())?
    }
}

impl AudioIo for LaneIo {
    fn start(&mut self, sink: IoSink) -> Result<(), String> {
        let mut state = self.inner.state.lock().expect("shared audio");
        let Some(lane) = state.lanes.get_mut(&self.id) else {
            return Err("audio-lane-gone".into());
        };
        lane.sink = Some(sink.clone());
        lane.incarnation = lane.incarnation.wrapping_add(1);
        if !state.running {
            state.generation += 1;
            let physical_sink = IoSink {
                messages: self.inner.messages.clone(),
                generation: state.generation,
                rendered: Arc::clone(&self.inner.rendered),
                direct_rendered: true,
            };
            if let Err(code) = state.physical.start(physical_sink) {
                state.lanes.get_mut(&self.id).expect("lane").sink = None;
                return Err(code);
            }
            state.running = true;
        }
        sink.send(IoEvent::Ready);
        Ok(())
    }

    fn play(&mut self, utterance: &str, chunk: usize, samples: Vec<f32>, sample_rate: u32) {
        let mut state = self.inner.state.lock().expect("shared audio");
        let Some(lane) = state.lanes.get_mut(&self.id) else {
            return;
        };
        if lane.sink.is_none() {
            return;
        }
        let mut resampled = Vec::new();
        Resampler::new(sample_rate, RATE).process(&samples, &mut resampled);
        lane.stopping = false;
        lane.queued.push_back(Chunk {
            utterance: utterance.to_owned(),
            chunk,
            samples: resampled,
            cursor: 0,
        });
        state.prime();
    }

    fn stop_playback(&mut self) {
        let mut state = self.inner.state.lock().expect("shared audio");
        let pending = state
            .frames
            .values()
            .any(|frame| frame.parts.iter().any(|part| part.lane == self.id));
        if let Some(lane) = state.lanes.get_mut(&self.id) {
            lane.queued.clear();
            lane.stopping = pending;
            if !pending {
                if let Some(sink) = &lane.sink {
                    sink.send(IoEvent::PlaybackEnded);
                }
            }
        }
    }

    fn stop(&mut self) {
        let mut state = self.inner.state.lock().expect("shared audio");
        if let Some(lane) = state.lanes.get_mut(&self.id) {
            lane.sink = None;
            lane.incarnation = lane.incarnation.wrapping_add(1);
            lane.queued.clear();
            lane.stopping = false;
        }
        if state.routed == Some(self.id) {
            state.routed = None;
        }
        if state.running && state.lanes.values().all(|lane| lane.sink.is_none()) {
            state.physical.stop();
            state.running = false;
            state.generation += 1;
            state.frames.clear();
            state.frame_order.clear();
            state.rendered_pending.clear();
        }
    }
}

impl Drop for LaneIo {
    fn drop(&mut self) {
        self.stop();
        self.inner
            .state
            .lock()
            .expect("shared audio")
            .lanes
            .remove(&self.id);
    }
}

impl State {
    /// Attribute only the physical samples actually consumed by the output. The physical backend reports rendered
    /// samples before progress in a tick, so hold them until its first chunk-start marker arrives.
    fn rendered(&mut self, samples: Vec<f32>, queued: Vec<bool>) {
        if self.frame_order.is_empty() {
            return;
        }
        // A queued silent sample belongs to the frame. An unqueued silent sample is device idle time.
        // The callback reports this distinction explicitly, even when Rendered precedes ChunkStarted.
        self.rendered_pending.extend(
            samples
                .into_iter()
                .zip(queued)
                .filter_map(|(sample, queued)| queued.then_some(sample)),
        );
        self.drain_rendered();
    }

    fn drain_rendered(&mut self) {
        while let Some(id) = self.frame_order.front().cloned() {
            let Some(frame) = self.frames.get_mut(&id) else {
                self.frame_order.pop_front();
                continue;
            };
            if !frame.started || self.rendered_pending.is_empty() {
                break;
            }
            let start = frame.rendered;
            let count = (FRAME - start).min(self.rendered_pending.len());
            let physical: Vec<f32> = self.rendered_pending.drain(..count).collect();
            frame.rendered += count;
            for (&lane_id, lane) in &self.lanes {
                let Some(sink) = &lane.sink else { continue };
                let part = frame
                    .parts
                    .iter()
                    .find(|part| part.lane == lane_id && part.incarnation == lane.incarnation);
                let Some(part) = part else { continue };
                let attributed = (0..count)
                    .map(|offset| {
                        let at = start + offset;
                        let mixed = frame.mixed[at];
                        if mixed.abs() <= 1.0e-6 {
                            0.0
                        } else {
                            physical[offset] * part.samples[at] / mixed
                        }
                    })
                    .collect();
                sink.send(IoEvent::Rendered(attributed));
            }
            if frame.rendered == FRAME {
                let played = frame.played;
                self.frame_order.pop_front();
                if played {
                    self.frames.remove(&id);
                    self.finish_stops();
                    self.prime();
                }
            }
        }
    }

    fn finish_stops(&mut self) {
        for (&id, lane) in &mut self.lanes {
            if lane.stopping
                && !self.frames.values().any(|frame| {
                    frame
                        .parts
                        .iter()
                        .any(|part| part.lane == id && part.incarnation == lane.incarnation)
                })
            {
                lane.stopping = false;
                if let Some(sink) = &lane.sink {
                    sink.send(IoEvent::PlaybackEnded);
                }
            }
        }
    }

    fn prime(&mut self) {
        while self.running && self.frames.len() < MIX_AHEAD {
            let mut parts: Vec<Part> = self
                .lanes
                .iter_mut()
                .filter_map(|(&id, lane)| {
                    let current = lane.queued.front_mut()?;
                    let first = current.cursor == 0;
                    let end = (current.cursor + FRAME).min(current.samples.len());
                    let mut samples = current.samples[current.cursor..end].to_vec();
                    samples.resize(FRAME, 0.0);
                    current.cursor = end;
                    let last = current.cursor == current.samples.len();
                    let part = Part {
                        lane: id,
                        incarnation: lane.incarnation,
                        utterance: current.utterance.clone(),
                        chunk: current.chunk,
                        samples,
                        first,
                        last,
                    };
                    if last {
                        lane.queued.pop_front();
                    }
                    Some(part)
                })
                .collect();
            if parts.is_empty() {
                break;
            }
            let mut mixed = vec![0.0; FRAME];
            // Keep the AEC reference equal to the samples actually sent to the speaker.
            for (i, sample) in mixed.iter_mut().enumerate() {
                let sum: f32 = parts.iter().map(|part| part.samples[i]).sum();
                let scale = if sum.abs() > 1.0 {
                    1.0 / sum.abs()
                } else {
                    1.0
                };
                *sample = sum * scale;
                for part in &mut parts {
                    part.samples[i] *= scale;
                }
            }
            let id = format!("shared-frame-{}", self.next_frame);
            self.next_frame += 1;
            self.frames.insert(
                id.clone(),
                MixedFrame {
                    parts,
                    mixed: mixed.clone(),
                    rendered: 0,
                    started: false,
                    played: false,
                },
            );
            self.frame_order.push_back(id.clone());
            self.physical.play(&id, 0, mixed, RATE);
        }
    }

    fn event(&mut self, event: IoEvent) {
        match event {
            IoEvent::Ready => {}
            IoEvent::Captured(samples) => {
                let silence = vec![0.0; samples.len()];
                for (&id, lane) in &self.lanes {
                    if let Some(sink) = &lane.sink {
                        sink.send(IoEvent::Captured(if self.routed == Some(id) {
                            samples.clone()
                        } else {
                            silence.clone()
                        }));
                    }
                }
            }
            IoEvent::Rendered(samples) => {
                let queued = vec![true; samples.len()];
                self.rendered(samples, queued);
            }
            IoEvent::PlaybackEnded => {
                self.frames.clear();
                self.frame_order.clear();
                self.rendered_pending.clear();
                for lane in self.lanes.values_mut() {
                    lane.stopping = false;
                    if let Some(sink) = &lane.sink {
                        sink.send(IoEvent::PlaybackEnded);
                    }
                }
                self.prime();
            }
            IoEvent::ChunkStarted { utterance, .. } => {
                if let Some(frame) = self.frames.get_mut(&utterance) {
                    frame.started = true;
                    for (&id, lane) in &self.lanes {
                        let Some(sink) = &lane.sink else { continue };
                        let part = frame
                            .parts
                            .iter()
                            .find(|part| part.lane == id && part.incarnation == lane.incarnation);
                        if let Some(part) = part.filter(|part| part.first) {
                            sink.send(IoEvent::ChunkStarted {
                                utterance: part.utterance.clone(),
                                chunk: part.chunk,
                            });
                        }
                    }
                }
                self.drain_rendered();
            }
            IoEvent::ChunkPlayed { utterance, .. } => {
                if let Some(frame) = self.frames.get_mut(&utterance) {
                    frame.played = true;
                    for part in &frame.parts {
                        if part.last {
                            if let Some(sink) = self
                                .lanes
                                .get(&part.lane)
                                .filter(|lane| lane.incarnation == part.incarnation)
                                .and_then(|lane| lane.sink.as_ref())
                            {
                                sink.send(IoEvent::ChunkPlayed {
                                    utterance: part.utterance.clone(),
                                    chunk: part.chunk,
                                });
                            }
                        }
                    }
                    if frame.rendered == FRAME {
                        self.frames.remove(&utterance);
                        self.finish_stops();
                        self.prime();
                    }
                }
            }
            IoEvent::MicrophoneMuted(muted) => {
                for lane in self.lanes.values() {
                    if let Some(sink) = &lane.sink {
                        sink.send(IoEvent::MicrophoneMuted(muted));
                    }
                }
            }
            IoEvent::Failed(code) => {
                self.running = false;
                self.frames.clear();
                self.frame_order.clear();
                self.rendered_pending.clear();
                self.routed = None;
                for lane in self.lanes.values_mut() {
                    if let Some(sink) = lane.sink.take() {
                        sink.send(IoEvent::Failed(code.clone()));
                    }
                    lane.queued.clear();
                }
            }
        }
    }
}

async fn dispatch(inner: Weak<Inner>, mut received: UnboundedReceiver<Message>) {
    while let Some(message) = received.next().await {
        let Some(inner) = inner.upgrade() else { break };
        let mut state = inner.state.lock().expect("shared audio");
        match message {
            Message::Io(generation, event) if generation == state.generation => state.event(event),
            Message::PhysicalRendered(generation, queued, samples)
                if generation == state.generation =>
            {
                state.rendered(samples, queued)
            }
            Message::SharedRoute(id, answer) => {
                let result =
                    if id == 0 || state.lanes.get(&id).is_some_and(|lane| lane.sink.is_some()) {
                        state.routed = (id != 0).then_some(id);
                        Ok(())
                    } else {
                        Err("audio-input-inactive".into())
                    };
                let _ = answer.send(result);
            }
            Message::Rendered(generation) => {
                let _ = inner.rendered.lock().expect("rendered").take(generation);
            }
            _ => {}
        }
    }
}
