//! The speaker's side of the native audio thread (`Playout`): what it is told to play, resampled to the speaker's rate
//! by one resampler that keeps its state from chunk to chunk, queued into the playback ring as room frees up, what the
//! callback took metered back at 16 kHz, and when each chunk starts and ends at the speaker.
//!
//! One resampler for the whole stream, never one per chunk: a fresh one would start each chunk from silence and stop
//! one input sample short of its end, a click at every boundary and chunks a little shorter than they were.

use std::sync::atomic::Ordering;

use rtrb::{Consumer, Producer};

use super::drain;
use super::output::{Counters, Queue, ReferenceSample, Schedule};
use super::pipeline::Pipeline;
use crate::audio::Resampler;
use crate::io::{IoEvent, IoSink};

/// What the audio thread is told to do with the speaker, in order.
pub(super) enum Command {
    /// Queue a chunk: mono samples at `sample_rate`.
    Play {
        utterance: String,
        chunk: usize,
        samples: Vec<f32>,
        sample_rate: u32,
    },
    /// Stop with a fade and drop what is queued.
    Stop,
}

/// The speaker's side of one open speaker.
pub(super) struct Playout {
    /// The speaker's rate.
    rate: u32,
    /// The stream's resampler, and the rate it takes.
    resampler: Option<(u32, Resampler)>,
    pub(super) queue: Queue,
    /// Where a stop's flush ends, until it has played.
    ending: Option<u64>,
    had_chunks: bool,
    drained_reference: u64,
    played: Vec<ReferenceSample>,
}

impl Playout {
    /// The speaker's side of a speaker at `rate`.
    pub(super) fn new(rate: u32) -> Self {
        Self {
            rate,
            resampler: None,
            queue: Queue::default(),
            ending: None,
            had_chunks: false,
            drained_reference: 0,
            played: Vec::new(),
        }
    }

    /// Does what it is told. A stop also starts the resampler over: what comes after it does not follow what was cut.
    pub(super) fn command(&mut self, command: Command, counters: &Counters) {
        match command {
            Command::Play {
                utterance,
                chunk,
                samples,
                sample_rate,
            } => {
                let rate = self.rate;
                let resampler = match &mut self.resampler {
                    Some((from, resampler)) if *from == sample_rate => resampler,
                    slot => {
                        &mut slot
                            .insert((sample_rate, Resampler::new(sample_rate, rate)))
                            .1
                    }
                };
                let mut resampled = Vec::with_capacity(
                    (samples.len() as u64 * u64::from(rate) / u64::from(sample_rate.max(1)))
                        as usize
                        + 2,
                );
                resampler.process(&samples, &mut resampled);
                self.had_chunks = true;
                self.ending = None;
                self.queue.play(utterance, chunk, resampled);
            }
            Command::Stop => {
                self.ending = Some(self.queue.stop(counters));
                self.resampler = None;
            }
        }
    }

    /// Moves what waits into the playback ring, and takes what the callback played from the reference ring.
    pub(super) fn exchange(
        &mut self,
        playback: &mut Producer<f32>,
        reference: &mut Consumer<ReferenceSample>,
    ) {
        self.queue.feed(playback);
        drain(reference, &mut self.played, 1);
        self.drained_reference += self.played.len() as u64;
    }

    /// Passes what was played to the echo canceller as its reference, and its meter to `sink` while anything plays.
    pub(super) fn render(&mut self, pipeline: &mut Pipeline, sink: &IoSink) {
        let played: Vec<f32> = self.played.iter().map(|sample| sample.value).collect();
        let queued: Vec<f32> = self
            .played
            .iter()
            .map(|sample| f32::from(u8::from(sample.queued)))
            .collect();
        let rendered = pipeline.render(&played);
        let presence = pipeline.render_presence(&queued);
        if (self.had_chunks || self.ending.is_some()) && !rendered.is_empty() {
            sink.send_physical_rendered(rendered, presence);
        }
    }

    /// Reports every chunk boundary the callback passed, then `captured`, then the end of playback once nothing is left
    /// to play.
    pub(super) fn report(
        &mut self,
        counters: &Counters,
        captured: impl IntoIterator<Item = Vec<f32>>,
        pipeline: &mut Pipeline,
        sink: &IoSink,
    ) {
        let consumed = counters.consumed.load(Ordering::Acquire);
        // The callback publishes its consumed count after writing the reference. Report every playback boundary
        // visible at this snapshot before the capture from this tick can qualify a turn and freeze its extent.
        send_capture_after_playback(&mut self.queue.schedule, consumed, captured, |event| {
            sink.send(event);
        });
        // `consumed` is published after the callback's reference writes. A callback may run between the drain and
        // this load; wait until its published reference tail has crossed the worker before ending the meter.
        if playback_complete(
            self.ending,
            self.had_chunks,
            self.queue.is_idle(),
            consumed,
            counters.referenced.load(Ordering::Acquire),
            self.drained_reference,
        ) {
            sink.send(IoEvent::PlaybackEnded);
            pipeline.reset_playback_meter();
            self.ending = None;
            self.had_chunks = false;
        }
    }
}

pub(super) fn send_capture_after_playback(
    schedule: &mut Schedule,
    consumed: u64,
    frames: impl IntoIterator<Item = Vec<f32>>,
    mut send: impl FnMut(IoEvent),
) {
    for event in schedule.due(consumed) {
        send(event);
    }
    for frame in frames {
        send(IoEvent::Captured(frame));
    }
}

/// Completion is behind the callback's published reference tail, even if it ran after this tick's drain.
pub(super) fn playback_complete(
    ending: Option<u64>,
    had_chunks: bool,
    idle: bool,
    consumed: u64,
    referenced: u64,
    drained_reference: u64,
) -> bool {
    (ending.is_some_and(|end| consumed >= end) || (had_chunks && idle && ending.is_none()))
        && drained_reference >= referenced
}
