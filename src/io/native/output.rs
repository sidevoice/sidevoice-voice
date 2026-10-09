//! The speaker's side of the native microphone and speaker: what the output callback plays (`Output`), what waits
//! for room in the playback ring (`Queue`), and which chunk is where in what it played (`Schedule`).
//!
//! The playback ring holds mono samples at the output's rate, chunk after chunk. The callback takes them one by one;
//! the samples it takes are also the echo canceller's reference, pushed to the reference ring as they are taken, and
//! their count is the clock of the heard position. Stopping fades out over a few milliseconds, then drops what was
//! queued before the stop and nothing after it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

use rtrb::{Consumer, Producer};

use crate::io::IoEvent;

/// The counters the callback and the rest share, in samples at the output's rate.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    /// Samples the callback has taken.
    pub(crate) consumed: AtomicU64,
    /// Every sample queued before this count is dropped (after the fade).
    pub(crate) flush_to: AtomicU64,
}

/// What the output callback runs: one sample at a time, no lock and no allocation.
pub(crate) struct Output {
    ring: Consumer<f32>,
    reference: Producer<f32>,
    fade: usize,
    fading: Option<usize>,
    consumed: u64,
}

impl Output {
    /// Plays from `ring` and copies what it plays to `reference`, fading out over `fade` samples on a stop.
    pub(crate) fn new(ring: Consumer<f32>, reference: Producer<f32>, fade: usize) -> Self {
        Self {
            ring,
            reference,
            fade: fade.max(1),
            fading: None,
            consumed: 0,
        }
    }

    /// The next sample to play.
    pub(crate) fn next(&mut self, counters: &Counters) -> f32 {
        let flush_to = counters.flush_to.load(Ordering::Acquire);
        if self.consumed < flush_to && self.fading.is_none() {
            self.fading = Some(self.fade);
        }
        let sample = match self.fading {
            Some(left) if left == 0 || self.consumed >= flush_to => {
                let dropped =
                    (flush_to.saturating_sub(self.consumed) as usize).min(self.ring.slots());
                if let Ok(chunk) = self.ring.read_chunk(dropped) {
                    chunk.commit_all();
                    self.consumed += dropped as u64;
                }
                self.fading = None;
                0.0
            }
            Some(left) => {
                self.fading = Some(left - 1);
                self.take() * left as f32 / self.fade as f32
            }
            None => self.take(),
        };
        let _ = self.reference.push(sample);
        sample
    }

    /// The next queued sample, or silence.
    fn take(&mut self) -> f32 {
        match self.ring.pop() {
            Ok(sample) => {
                self.consumed += 1;
                sample
            }
            Err(_) => 0.0,
        }
    }

    /// Publishes how many samples were taken.
    pub(crate) fn publish(&self, counters: &Counters) {
        counters.consumed.store(self.consumed, Ordering::Release);
    }
}

/// Where a chunk sits in the playback ring, in samples.
#[derive(Debug, Clone, PartialEq)]
struct Placed {
    utterance: String,
    chunk: usize,
    start: u64,
    end: u64,
    started: bool,
}

/// The chunks queued for playback, in order, until each has played.
#[derive(Debug, Default)]
pub(crate) struct Schedule {
    chunks: VecDeque<Placed>,
}

impl Schedule {
    /// A chunk queued from sample `start` to just before `end`.
    pub(crate) fn place(&mut self, utterance: &str, chunk: usize, start: u64, end: u64) {
        self.chunks.push_back(Placed {
            utterance: utterance.to_owned(),
            chunk,
            start,
            end,
            started: false,
        });
    }

    /// Forgets every chunk: they were stopped.
    pub(crate) fn clear(&mut self) {
        self.chunks.clear();
    }

    /// What became of the chunks once `consumed` samples were taken: each starts when its first sample is taken and
    /// has played when its last one is.
    pub(crate) fn due(&mut self, consumed: u64) -> Vec<IoEvent> {
        let mut events = Vec::new();
        while let Some(placed) = self.chunks.front_mut() {
            // An empty chunk starts as it ends.
            if !placed.started && (consumed > placed.start || consumed >= placed.end) {
                placed.started = true;
                events.push(IoEvent::ChunkStarted {
                    utterance: placed.utterance.clone(),
                    chunk: placed.chunk,
                });
            }
            if consumed < placed.end {
                break;
            }
            events.push(IoEvent::ChunkPlayed {
                utterance: placed.utterance.clone(),
                chunk: placed.chunk,
            });
            self.chunks.pop_front();
        }
        events
    }
}

/// A chunk waiting to be wholly in the playback ring.
#[derive(Debug)]
struct Pending {
    utterance: String,
    chunk: usize,
    samples: Vec<f32>,
    /// How many of its samples are in the ring, and the count its first one was queued at.
    pushed: usize,
    start: Option<u64>,
}

/// What the audio thread queues for the speaker: chunks go into the playback ring as room frees up, never in part
/// and never dropped, and each is placed in the schedule once its last sample is in, so a chunk is never reported
/// played before all of it was taken.
#[derive(Debug, Default)]
pub(crate) struct Queue {
    pending: VecDeque<Pending>,
    /// Samples ever pushed into the ring.
    produced: u64,
    pub(crate) schedule: Schedule,
}

impl Queue {
    /// Queues a chunk's samples, at the output's rate.
    pub(crate) fn play(&mut self, utterance: String, chunk: usize, samples: Vec<f32>) {
        self.pending.push_back(Pending {
            utterance,
            chunk,
            samples,
            pushed: 0,
            start: None,
        });
    }

    /// Stops: what waits is dropped, the schedule forgotten, and the callback drops what the ring holds.
    pub(crate) fn stop(&mut self, counters: &Counters) {
        self.pending.clear();
        self.schedule.clear();
        counters.flush_to.store(self.produced, Ordering::Release);
    }

    /// Moves what waits into `ring`, as much as it has room for, in order.
    pub(crate) fn feed(&mut self, ring: &mut Producer<f32>) {
        while let Some(pending) = self.pending.front_mut() {
            let start = *pending.start.get_or_insert(self.produced);
            while pending.pushed < pending.samples.len() {
                if ring.push(pending.samples[pending.pushed]).is_err() {
                    return;
                }
                pending.pushed += 1;
                self.produced += 1;
            }
            self.schedule
                .place(&pending.utterance, pending.chunk, start, self.produced);
            self.pending.pop_front();
        }
    }
}
