//! Several call-local audio lanes over one microphone, speaker and echo canceller. The physical speaker receives
//! short mixed frames, kept a few tens of milliseconds ahead of what it plays; its actual playback clock drives each
//! lane's chunk and rendered reports.
//!
//! A lane's playback ends when everything it was given has played, never when the physical speaker momentarily runs
//! dry: a frame sent is a frame reported. Stopping a lane cuts the speaker at once (with its short fade) and mixes
//! what the other lanes had in flight again, so an interruption is never late by the lead.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
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
/// How many mixed 10 ms frames wait at the speaker ahead of what it plays: enough to ride out the audio thread's
/// wake-ups and a busy dispatcher without running dry, and no more, since a stop cuts them all.
const MIX_AHEAD: usize = 8;

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
    /// The mixed frames sent to the speaker and not played yet.
    frames: HashMap<String, MixedFrame>,
    /// The frames whose rendered samples are not all in yet, in order.
    frame_order: VecDeque<String>,
    rendered_pending: VecDeque<f32>,
}

struct Lane {
    sink: Option<IoSink>,
    queued: VecDeque<Chunk>,
    incarnation: u64,
    /// The lane's one resampler to the mixing rate, and the rate it takes: chunks follow each other without a seam.
    resampler: Option<(u32, Resampler)>,
    /// Whether it has played anything since its playback last ended.
    sounding: bool,
    /// Source-level pending fence; a later synthesized chunk with this id is discarded.
    fenced: HashSet<String>,
    /// Identities handed to the physical output, which may already be audible
    /// even before its start callback; retained across delayed actor events.
    started: HashSet<String>,
    fence_overflow: bool,
}

struct Chunk {
    utterance: String,
    chunk: usize,
    samples: Arc<[f32]>,
    cursor: usize,
    /// Whether its start was already reported (a chunk mixed again after a stop of another lane).
    announced: bool,
}

/// One lane's frame of a mix: its samples, cut from one chunk or running on into the next, so that chunks follow each
/// other without a gap.
struct Part {
    lane: u64,
    incarnation: u64,
    samples: Vec<f32>,
    pieces: Vec<Piece>,
}

/// The run of one chunk's samples in a part.
struct Piece {
    utterance: String,
    chunk: usize,
    /// The chunk it was cut from, and where.
    source: Arc<[f32]>,
    offset: usize,
    first: bool,
    last: bool,
}

impl Part {
    /// Whether the chunk of `piece` ends before the part does: such an end is reported as the frame starts, so that
    /// it comes before the next chunk's start.
    fn ends_inside(&self, piece: usize) -> bool {
        self.pieces[piece].last && piece + 1 < self.pieces.len()
    }
}

struct MixedFrame {
    seq: u64,
    parts: Vec<Part>,
    mixed: Vec<f32>,
    rendered: usize,
    started: bool,
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
                incarnation: 0,
                resampler: None,
                sounding: false,
                fenced: HashSet::new(),
                started: HashSet::new(),
                fence_overflow: false,
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

impl InputPort {
    /// Atomically fence only a saying with no frame already handed to the
    /// physical output and no earlier started frame. An in-flight frame may
    /// already be audible even if its callback is late, so it is left alone.
    pub fn fence_pending(&self, utterance: &str) -> bool {
        let mut state = self.inner.state.lock().expect("shared audio");
        let Some(lane) = state.lanes.get(&self.id) else {
            return false;
        };
        if lane.fence_overflow || lane.started.contains(utterance) {
            return false;
        }
        let incarnation = lane.incarnation;
        if state.frames.values().any(|frame| {
            frame.parts.iter().any(|part| {
                part.lane == self.id
                    && part.incarnation == incarnation
                    && part.pieces.iter().any(|piece| piece.utterance == utterance)
            })
        }) {
            return false;
        }
        let lane = state.lanes.get_mut(&self.id).expect("lane");
        if lane.fenced.len() >= 2048 {
            lane.fence_overflow = true;
            return false;
        }
        lane.fenced.insert(utterance.to_owned());
        lane.queued.retain(|chunk| chunk.utterance != utterance);
        // No resampler tail from a never-played chunk may leak into the next.
        lane.resampler = None;
        true
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
        // A pending fence can be acknowledged before the lane first starts.
        // Keep saying IDs for this lane's lifetime across starts and stops;
        // delayed bridge commands must see the same source decision.
        if !state.running {
            state.generation += 1;
            let physical_sink = IoSink {
                messages: self.inner.messages.clone(),
                generation: state.generation,
                rendered: Arc::clone(&self.inner.rendered),
                direct_rendered: true,
                capture: None,
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
        if lane.fenced.contains(utterance) {
            return;
        }
        let resampler = match &mut lane.resampler {
            Some((from, resampler)) if *from == sample_rate => resampler,
            slot => {
                &mut slot
                    .insert((sample_rate, Resampler::new(sample_rate, RATE)))
                    .1
            }
        };
        let mut resampled = Vec::new();
        resampler.process(&samples, &mut resampled);
        lane.queued.push_back(Chunk {
            utterance: utterance.to_owned(),
            chunk,
            samples: resampled.into(),
            cursor: 0,
            announced: false,
        });
        state.prime();
    }

    fn stop_playback(&mut self) {
        let mut state = self.inner.state.lock().expect("shared audio");
        let Some(lane) = state.lanes.get_mut(&self.id) else {
            return;
        };
        lane.queued.clear();
        lane.resampler = None;
        lane.sounding = false;
        let sink = lane.sink.clone();
        if state.in_flight(self.id) {
            state.flush(self.id);
        }
        if let Some(sink) = sink {
            sink.send(IoEvent::PlaybackEnded);
        }
    }

    fn stop(&mut self) {
        let mut state = self.inner.state.lock().expect("shared audio");
        let in_flight = state.in_flight(self.id);
        if let Some(lane) = state.lanes.get_mut(&self.id) {
            lane.sink = None;
            lane.incarnation = lane.incarnation.wrapping_add(1);
            lane.queued.clear();
            lane.resampler = None;
            lane.sounding = false;
            // A Tauri pending-cancel command may arrive after stop. Keep the
            // bounded identity history until this lane is dropped, so a
            // previously handed-off saying cannot become "never started".
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
        } else if in_flight {
            state.flush(self.id);
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
    fn note_started(&mut self, id: u64, incarnation: u64, utterance: &str) {
        let Some(lane) = self
            .lanes
            .get_mut(&id)
            .filter(|lane| lane.incarnation == incarnation)
        else {
            return;
        };
        if lane.fence_overflow {
            return;
        }
        lane.started.insert(utterance.to_owned());
        if lane.started.len() >= 2048 {
            lane.fence_overflow = true;
            lane.started.clear()
        }
    }

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

    /// Hands the rendered samples to the lanes, frame after frame. A frame that played before all of its samples came
    /// back leaves the rest to the next one; the meter is not worth holding a played frame for.
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
                self.frame_order.pop_front();
            }
        }
        // Nothing left to attribute them to: what is left is the tail of frames that already played.
        if self.frame_order.is_empty() {
            self.rendered_pending.clear();
        }
    }

    /// Whether the speaker holds anything of the lane `id`'s current incarnation.
    fn in_flight(&self, id: u64) -> bool {
        let Some(lane) = self.lanes.get(&id) else {
            return false;
        };
        self.frames.values().any(|frame| {
            frame
                .parts
                .iter()
                .any(|part| part.lane == id && part.incarnation == lane.incarnation)
        })
    }

    /// Ends the playback of every lane that played and has nothing left, queued or at the speaker.
    fn end_idle_lanes(&mut self) {
        let ended: Vec<u64> = self
            .lanes
            .iter()
            .filter(|(&id, lane)| lane.sounding && lane.queued.is_empty() && !self.in_flight(id))
            .map(|(&id, _)| id)
            .collect();
        for id in ended {
            let lane = self.lanes.get_mut(&id).expect("lane");
            lane.sounding = false;
            if let Some(sink) = &lane.sink {
                sink.send(IoEvent::PlaybackEnded);
            }
        }
    }

    /// Cuts the speaker (with its fade) and drops every frame it holds, then mixes again what the other lanes than
    /// `except` had in them, from the start of the oldest frame that had not played: a few milliseconds of theirs may
    /// sound twice, none is lost, and every chunk still starts and ends once.
    fn flush(&mut self, except: u64) {
        self.physical.stop_playback();
        let mut frames: Vec<MixedFrame> = self.frames.drain().map(|(_, frame)| frame).collect();
        frames.sort_by_key(|frame| frame.seq);
        self.frame_order.clear();
        self.rendered_pending.clear();
        let mut restored: BTreeMap<u64, Vec<Chunk>> = BTreeMap::new();
        for frame in frames {
            for part in frame.parts {
                let current = part.lane != except
                    && self.lanes.get(&part.lane).is_some_and(|lane| {
                        lane.sink.is_some() && lane.incarnation == part.incarnation
                    });
                if !current {
                    continue;
                }
                let chunks = restored.entry(part.lane).or_default();
                let reported: Vec<bool> = (0..part.pieces.len())
                    .map(|piece| frame.started && part.ends_inside(piece))
                    .collect();
                for (piece, reported) in part.pieces.into_iter().zip(reported) {
                    // Its end is already told: what little of it had not sounded counts as played.
                    if reported
                        || chunks
                            .last()
                            .is_some_and(|chunk| Arc::ptr_eq(&chunk.samples, &piece.source))
                    {
                        continue;
                    }
                    chunks.push(Chunk {
                        utterance: piece.utterance,
                        chunk: piece.chunk,
                        samples: piece.source,
                        cursor: piece.offset,
                        announced: !piece.first || frame.started,
                    });
                }
            }
        }
        for (id, chunks) in restored {
            let lane = self.lanes.get_mut(&id).expect("lane");
            let continues = lane
                .queued
                .front()
                .zip(chunks.last())
                .is_some_and(|(next, last)| Arc::ptr_eq(&next.samples, &last.samples));
            if continues {
                lane.queued.pop_front();
            }
            for chunk in chunks.into_iter().rev() {
                lane.queued.push_front(chunk);
            }
        }
        self.prime();
    }

    /// Mixes and sends frames until the speaker holds [`MIX_AHEAD`] of them or no lane has more.
    fn prime(&mut self) {
        while self.running && self.frames.len() < MIX_AHEAD {
            let mut parts: Vec<Part> = self
                .lanes
                .iter_mut()
                .filter_map(|(&id, lane)| {
                    let mut samples = Vec::with_capacity(FRAME);
                    let mut pieces = Vec::new();
                    while samples.len() < FRAME {
                        let Some(current) = lane.queued.front_mut() else {
                            break;
                        };
                        let offset = current.cursor;
                        let end = (offset + FRAME - samples.len()).min(current.samples.len());
                        samples.extend_from_slice(&current.samples[offset..end]);
                        current.cursor = end;
                        let last = end == current.samples.len();
                        pieces.push(Piece {
                            utterance: current.utterance.clone(),
                            chunk: current.chunk,
                            source: Arc::clone(&current.samples),
                            offset,
                            first: offset == 0 && !current.announced,
                            last,
                        });
                        if last {
                            lane.queued.pop_front();
                        }
                    }
                    if pieces.is_empty() {
                        return None;
                    }
                    // Only a lane with nothing more queued leaves a gap.
                    samples.resize(FRAME, 0.0);
                    lane.sounding = true;
                    Some(Part {
                        lane: id,
                        incarnation: lane.incarnation,
                        samples,
                        pieces,
                    })
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
            let seq = self.next_frame;
            self.next_frame += 1;
            let id = format!("shared-frame-{seq}");
            // Record potential physical start before handing the frame off.
            // A late ChunkStarted callback can arrive after stop/flush has
            // removed the frame, but cannot make a later fence certify zero.
            for part in &parts {
                for piece in &part.pieces {
                    self.note_started(part.lane, part.incarnation, &piece.utterance);
                }
            }
            self.frames.insert(
                id.clone(),
                MixedFrame {
                    seq,
                    parts,
                    mixed: mixed.clone(),
                    rendered: 0,
                    started: false,
                },
            );
            self.frame_order.push_back(id.clone());
            self.physical.play(&id, 0, mixed, RATE);
        }
    }

    fn event(&mut self, event: IoEvent) {
        let captured_at = if let IoEvent::CapturedAt(_, at) = &event {
            Some(*at)
        } else {
            None
        };
        match event {
            IoEvent::Ready => {}
            IoEvent::Captured(samples) | IoEvent::CapturedAt(samples, _) => {
                let silence = vec![0.0; samples.len()];
                for (&id, lane) in &self.lanes {
                    if let Some(sink) = &lane.sink {
                        let samples = if self.routed == Some(id) {
                            samples.clone()
                        } else {
                            silence.clone()
                        };
                        sink.send(match captured_at {
                            Some(at) => IoEvent::CapturedAt(samples, at),
                            None => IoEvent::Captured(samples),
                        });
                    }
                }
            }
            IoEvent::Rendered(samples) => {
                let queued = vec![true; samples.len()];
                self.rendered(samples, queued);
            }
            // The speaker ran dry: between mixed frames that is an underrun, not an end. Every frame sent is still
            // reported as it plays, and each lane ends once all it was given has played.
            IoEvent::PlaybackEnded => {}
            IoEvent::ChunkStarted { utterance, .. } => {
                if let Some(frame) = self.frames.get_mut(&utterance) {
                    let started = frame
                        .parts
                        .iter()
                        .flat_map(|part| {
                            part.pieces.iter().map(move |piece| {
                                (part.lane, part.incarnation, piece.utterance.clone())
                            })
                        })
                        .collect::<Vec<_>>();
                    frame.started = true;
                    for (&id, lane) in &self.lanes {
                        let Some(sink) = &lane.sink else { continue };
                        let part = frame
                            .parts
                            .iter()
                            .find(|part| part.lane == id && part.incarnation == lane.incarnation);
                        let Some(part) = part else { continue };
                        for (index, piece) in part.pieces.iter().enumerate() {
                            if piece.first {
                                sink.send(IoEvent::ChunkStarted {
                                    utterance: piece.utterance.clone(),
                                    chunk: piece.chunk,
                                });
                            }
                            if part.ends_inside(index) {
                                sink.send(IoEvent::ChunkPlayed {
                                    utterance: piece.utterance.clone(),
                                    chunk: piece.chunk,
                                });
                            }
                        }
                    }
                    for (id, incarnation, name) in started {
                        self.note_started(id, incarnation, &name)
                    }
                }
                self.drain_rendered();
            }
            IoEvent::ChunkPlayed { utterance, .. } => {
                if let Some(frame) = self.frames.remove(&utterance) {
                    for part in &frame.parts {
                        for piece in &part.pieces {
                            self.note_started(part.lane, part.incarnation, &piece.utterance)
                        }
                    }
                    for part in &frame.parts {
                        let ending = part.pieces.last().filter(|piece| piece.last);
                        let sink = self
                            .lanes
                            .get(&part.lane)
                            .filter(|lane| lane.incarnation == part.incarnation)
                            .and_then(|lane| lane.sink.as_ref());
                        if let Some((piece, sink)) = ending.zip(sink) {
                            sink.send(IoEvent::ChunkPlayed {
                                utterance: piece.utterance.clone(),
                                chunk: piece.chunk,
                            });
                        }
                    }
                    self.end_idle_lanes();
                    self.prime();
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
                    lane.resampler = None;
                    lane.sounding = false;
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
