//! The call as a pure state machine: three regions in parallel, driven by [`Input`]s and a monotonic time in
//! milliseconds, answering with [`Effect`]s. It holds no socket, no clock and no thread: whoever drives it runs the
//! models and the speaker, and feeds back what they did.
//!
//! - **Listening** (`turns`): `Idle → Listening ⇄ Speaking`, and back to `Listening` when a turn ends. A turn's audio
//!   goes to recognition.
//! - **Recognition** (`recognition`): turns are transcribed one at a time, filtered, held for the merge window and
//!   reported finished (with the words), cancelled, or joined into the next one.
//! - **Playback** (`playback`): `Idle → Synthesizing → Playing → Idle`, one thing said after another. A turn that opens
//!   while something is on its way is a barge-in: it stops and everything queued is dropped. What is to be said waits
//!   while the person's turn is open or being transcribed, and for the grace after it.

#[cfg(test)]
mod tests;

use std::collections::{HashMap, VecDeque};

use crate::config::{EndOfTurn, VoiceConfig};
use crate::event::{
    CallState, Listening, Microphone, TurnEvent, TurnTimings, VoiceError, VoiceEvent,
};
use crate::playback::{Action, Playback};
use crate::recognition::{accepted, Job, Outcome, Recognition};
use crate::say::{SayEvent, SayOutcome, StopReason};
use crate::turns::{duration_ms, Segment, Segmentation, Segmenter};

/// What happens to the call.
#[derive(Debug)]
pub(crate) enum Input {
    /// Start listening.
    Start,
    /// Stop: the open turn and the turns not yet transcribed are cancelled, and what is being said stops.
    Stop,
    /// A new configuration.
    Config(Box<VoiceConfig>),
    /// One detector window: 16 kHz mono samples after echo cancellation, and whether the detector holds it for speech.
    Window { pcm: Vec<f32>, speech: bool },
    /// A turn's transcript, or the stable code of why there is none.
    Transcribed {
        turn: usize,
        result: Result<String, String>,
    },
    /// Say `text` under `id`, in `language` (the configuration's when absent).
    Say {
        id: String,
        text: String,
        language: Option<String>,
    },
    /// Cancel what is said under `id`.
    CancelSay(String),
    /// A chunk's speech (samples and their rate), or the stable code of why there is none.
    Synthesized {
        utterance: String,
        chunk: usize,
        result: Result<(Vec<f32>, u32), String>,
    },
    /// The output started sounding a chunk.
    ChunkStarted { utterance: String, chunk: usize },
    /// The output played a chunk's last sample.
    ChunkPlayed { utterance: String, chunk: usize },
    /// Whether the microphone is muted: an open turn ends with what was said.
    Mute(bool),
    /// Whether the microphone's device gives no audio for now (not the person's mute).
    MicrophoneMuted(bool),
    /// The person cancels what they said that is not reported yet.
    Cancel,
    /// What the end-of-turn model said of a turn's pause: the probability that the turn is over, or the stable code
    /// of why there is none.
    EndOfTurn {
        turn: usize,
        pause: u32,
        result: Result<f32, String>,
    },
}

/// How many cancels of ids not heard of yet are kept.
const CANCELS_KEPT: usize = 64;

/// The end-of-turn probability from which a paused turn is over.
const END_OF_TURN_LIKELY: f32 = 0.5;

/// What the driver must do.
#[derive(Debug, PartialEq)]
pub(crate) enum Effect {
    /// Tell the host.
    Event(VoiceEvent),
    /// Tell the handle of what is said under `id`.
    Say { id: String, event: SayEvent },
    /// Transcribe a turn's 16 kHz audio, then feed [`Input::Transcribed`].
    Transcribe {
        turn: usize,
        pcm: Vec<f32>,
        language: Option<String>,
    },
    /// Speak a chunk, then feed [`Input::Synthesized`].
    Synthesize {
        utterance: String,
        chunk: usize,
        text: String,
        language: Option<String>,
    },
    /// Queue a chunk's audio at the output, then feed [`Input::ChunkStarted`] and [`Input::ChunkPlayed`].
    Play {
        utterance: String,
        chunk: usize,
        samples: Vec<f32>,
        sample_rate: u32,
    },
    /// Stop the output with a short fade and drop what it holds.
    StopPlayback,
    /// Ask the end-of-turn model whether a paused turn is over, then feed [`Input::EndOfTurn`].
    EndOfTurn {
        turn: usize,
        pause: u32,
        pcm: Vec<f32>,
    },
}

/// A turn of the person's, until it is finished or cancelled.
#[derive(Debug)]
struct Turn {
    id: String,
    started_ms: u64,
    ended_ms: Option<u64>,
    audio_ms: u64,
    silence_ms: u64,
    recognised_ms: Option<u64>,
}

/// One call.
#[derive(Debug)]
pub(crate) struct Call {
    config: VoiceConfig,
    /// The prefix of every id this call makes.
    call_id: String,
    /// The Unix time of the call's millisecond 0.
    epoch_unix_ms: u64,
    started: bool,
    muted: bool,
    /// Whether the microphone's device gives no audio, until the call stops.
    microphone_muted: bool,
    segmenter: Segmenter,
    /// The turn being spoken.
    open: Option<usize>,
    turns: HashMap<usize, Turn>,
    next_turn: usize,
    recognition: Recognition,
    playback: Playback,
    /// Nothing starts to be said before this time (the grace after a turn).
    quiet_until: u64,
    /// Ids cancelled before the call heard of them: said later, they never play.
    cancelled: VecDeque<String>,
    state: Option<CallState>,
}

impl Call {
    pub(crate) fn new(config: VoiceConfig, call_id: String, epoch_unix_ms: u64) -> Self {
        Self {
            segmenter: Segmenter::new(segmentation(&config)),
            config,
            call_id,
            epoch_unix_ms,
            started: false,
            muted: false,
            microphone_muted: false,
            open: None,
            turns: HashMap::new(),
            next_turn: 0,
            recognition: Recognition::default(),
            playback: Playback::default(),
            quiet_until: 0,
            cancelled: VecDeque::new(),
            state: None,
        }
    }

    /// Takes one input at `now` (the call's monotonic milliseconds) and returns what to do.
    pub(crate) fn handle(&mut self, now: u64, input: Input) -> Vec<Effect> {
        let mut out = Vec::new();
        match input {
            Input::Start => self.started = true,
            Input::Stop => self.stop(now, &mut out),
            Input::Config(config) => {
                self.segmenter.set_numbers(segmentation(&config));
                self.config = *config;
            }
            Input::Window { pcm, speech } => self.window(now, &pcm, speech, &mut out),
            Input::Transcribed { turn, result } => self.transcribed(now, turn, result, &mut out),
            Input::Say { id, text, language } => self.say(id, &text, language, &mut out),
            Input::CancelSay(id) => {
                let mut actions = Vec::new();
                if !self.playback.cancel(&id, &mut actions) {
                    // Not here (yet): a cancel may overtake its own `say`, which then never plays.
                    self.cancelled.push_back(id);
                    if self.cancelled.len() > CANCELS_KEPT {
                        self.cancelled.pop_front();
                    }
                }
                self.act(actions, &mut out);
            }
            Input::Synthesized {
                utterance,
                chunk,
                result,
            } => {
                let mut actions = Vec::new();
                self.playback
                    .synthesized(&utterance, chunk, result, &mut actions);
                self.act(actions, &mut out);
            }
            Input::ChunkStarted { utterance, chunk } => {
                let mut actions = Vec::new();
                self.playback.chunk_started(&utterance, chunk, &mut actions);
                self.act(actions, &mut out);
            }
            Input::ChunkPlayed { utterance, chunk } => {
                let mut actions = Vec::new();
                self.playback.chunk_played(&utterance, chunk, &mut actions);
                self.act(actions, &mut out);
            }
            Input::Mute(muted) => {
                self.muted = muted;
                if muted {
                    if let Some(segment) = self.segmenter.close(0) {
                        self.segment(now, segment, &mut out);
                    }
                }
            }
            Input::MicrophoneMuted(muted) => self.microphone_muted = muted,
            Input::Cancel => self.cancel_input(now, &mut out),
            Input::EndOfTurn {
                turn,
                pause,
                result,
            } => match result {
                Ok(probability) if probability >= END_OF_TURN_LIKELY && self.open == Some(turn) => {
                    if let Some(segment) = self.segmenter.end_paused(pause) {
                        self.segment(now, segment, &mut out);
                    }
                }
                Ok(_) => {}
                Err(code) => self.error(&code, &mut out),
            },
        }
        self.settle(now, &mut out);
        out
    }

    /// The earliest time [`Call::poll`] has something to do, if any.
    pub(crate) fn deadline(&self) -> Option<u64> {
        let speaking = self.segmenter.speaking();
        // The grace is a deadline only when its end is all that holds the next one back.
        let say = (self.playback.waiting() && !self.playback.busy() && self.quiet(speaking))
            .then_some(self.quiet_until)
            .filter(|&until| until > 0);
        [
            self.segmenter.deadline(),
            self.recognition.deadline(speaking),
            say,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Does what is due at `now`: ends a turn whose audio stopped, reports a transcript whose merge window closed,
    /// starts saying what waited for the grace.
    pub(crate) fn poll(&mut self, now: u64) -> Vec<Effect> {
        let mut out = Vec::new();
        if let Some(segment) = self.segmenter.poll(now) {
            self.segment(now, segment, &mut out);
        }
        self.settle(now, &mut out);
        out
    }

    fn window(&mut self, now: u64, pcm: &[f32], speech: bool, out: &mut Vec<Effect>) {
        if !self.started || self.muted {
            return;
        }
        let playing = self.playback.sounding();
        if let Some(segment) = self.segmenter.window(now, pcm, speech, playing) {
            self.segment(now, segment, out);
        }
        out.push(Effect::Event(VoiceEvent::Level(self.segmenter.level())));
    }

    fn segment(&mut self, now: u64, segment: Segment, out: &mut Vec<Effect>) {
        match segment {
            Segment::Started => {
                let number = self.next_turn;
                self.next_turn += 1;
                let id = format!("{}-turn-{number}", self.call_id);
                self.turns.insert(
                    number,
                    Turn {
                        id: id.clone(),
                        started_ms: now,
                        ended_ms: None,
                        audio_ms: 0,
                        silence_ms: 0,
                        recognised_ms: None,
                    },
                );
                self.open = Some(number);
                out.push(Effect::Event(VoiceEvent::Turn(TurnEvent::Started {
                    turn_id: id,
                    started_at: self.epoch_unix_ms + now,
                })));
                let mut actions = Vec::new();
                self.playback.interrupt(StopReason::BargeIn, &mut actions);
                self.act(actions, out);
            }
            Segment::Paused { pcm, pause } => {
                if let Some(turn) = self.open {
                    out.push(Effect::EndOfTurn { turn, pause, pcm });
                }
            }
            Segment::Ended(ended) => {
                let Some(number) = self.open.take() else {
                    return;
                };
                if let Some(turn) = self.turns.get_mut(&number) {
                    turn.ended_ms = Some(now);
                    turn.audio_ms = duration_ms(ended.pcm.len());
                    turn.silence_ms = ended.silence_ms;
                }
                match self.recognition.push(Job {
                    turn: number,
                    pcm: ended.pcm,
                }) {
                    Ok(job) => self.transcribe(job, out),
                    Err(_) => {
                        self.error("transcription-queue-full", out);
                        self.close_turn(now, number, None, false, out);
                    }
                }
            }
        }
    }

    fn transcribe(&mut self, job: Option<Job>, out: &mut Vec<Effect>) {
        if let Some(job) = job {
            out.push(Effect::Transcribe {
                turn: job.turn,
                pcm: job.pcm,
                language: self.config.language.clone(),
            });
        }
    }

    fn transcribed(
        &mut self,
        now: u64,
        turn: usize,
        result: Result<String, String>,
        out: &mut Vec<Effect>,
    ) {
        if !self.recognition.is_active(turn) {
            return;
        }
        let next = self.recognition.done();
        let language = self.config.language.clone();
        let text = match result {
            Ok(text) => accepted(&text, language.as_deref()),
            Err(code) => {
                self.error(&code, out);
                None
            }
        };
        match text {
            None => self.close_turn(now, turn, None, false, out),
            Some(text) => {
                if let Some(record) = self.turns.get_mut(&turn) {
                    record.recognised_ms = Some(now);
                }
                let merge = u64::from(self.config.patience.merge_window_ms());
                if merge == 0 {
                    self.close_turn(now, turn, Some(text), false, out);
                } else if let Outcome::Joined(joined) =
                    self.recognition.hold(turn, text, now + merge)
                {
                    for earlier in joined {
                        self.close_turn(now, earlier, None, true, out);
                    }
                }
            }
        }
        self.transcribe(next, out);
    }

    fn cancel_input(&mut self, now: u64, out: &mut Vec<Effect>) {
        self.segmenter.cancel();
        let mut turns: Vec<usize> = self.open.take().into_iter().collect();
        turns.extend(self.recognition.clear());
        if let Some(pending) = self.recognition.take_pending() {
            turns.extend(pending.turns);
        }
        turns.sort_unstable();
        for turn in turns {
            self.close_turn(now, turn, None, false, out);
        }
    }

    fn stop(&mut self, now: u64, out: &mut Vec<Effect>) {
        self.cancel_input(now, out);
        let mut actions = Vec::new();
        self.playback.interrupt(StopReason::Stopped, &mut actions);
        self.act(actions, out);
        self.started = false;
        self.microphone_muted = false;
        // A stop is always answered with the state, even an unchanged one: the host knows it took effect.
        self.state = None;
    }

    /// Something to say: queued, or not played at all while the call is stopped.
    fn say(&mut self, id: String, text: &str, language: Option<String>, out: &mut Vec<Effect>) {
        let refused =
            if let Some(index) = self.cancelled.iter().position(|cancelled| *cancelled == id) {
                self.cancelled.remove(index);
                Some(StopReason::Cancelled)
            } else {
                (!self.started).then_some(StopReason::Stopped)
            };
        if let Some(reason) = refused {
            out.push(Effect::Say {
                id,
                event: SayEvent::Done {
                    outcome: SayOutcome::NotPlayed { reason },
                },
            });
            return;
        }
        let language = language.or_else(|| self.config.language.clone());
        let mut actions = Vec::new();
        self.playback.push(id, text, language, &mut actions);
        self.act(actions, out);
    }

    /// Whether nothing of the person's holds the playback back: the call listens and no turn is open, being
    /// transcribed or held for the merge window.
    fn quiet(&self, speaking: bool) -> bool {
        self.started
            && !speaking
            && self.open.is_none()
            && self.recognition.len() == 0
            && !self.recognition.holding()
    }

    /// Reports what is due and starts what may start, then the state if it changed.
    fn settle(&mut self, now: u64, out: &mut Vec<Effect>) {
        let speaking = self.segmenter.speaking();
        if let Some(pending) = self.recognition.due(now, speaking) {
            let (last, earlier) = pending.turns.split_last().expect("a held transcript");
            let merged = !earlier.is_empty();
            if let Some(first) = earlier.first().and_then(|first| self.turns.get(first)) {
                let started = first.started_ms;
                if let Some(turn) = self.turns.get_mut(last) {
                    turn.started_ms = started;
                }
            }
            self.close_turn(now, *last, Some(pending.text), merged, out);
        }
        if self.quiet(speaking) && now >= self.quiet_until && !self.playback.busy() {
            let mut actions = Vec::new();
            self.playback.start(&mut actions);
            self.act(actions, out);
        }
        let state = CallState {
            listening: if !self.started {
                Listening::Idle
            } else if self.muted {
                Listening::Muted
            } else if speaking {
                Listening::Speaking
            } else {
                Listening::Listening
            },
            recognising: self.recognition.len(),
            playback: self.playback.state(),
            microphone: if self.started && self.microphone_muted {
                Microphone::Muted
            } else {
                Microphone::Live
            },
        };
        if self.state != Some(state) {
            self.state = Some(state);
            out.push(Effect::Event(VoiceEvent::State(state)));
        }
    }

    /// Reports a turn's end, finished with `text` or cancelled without, and forgets it; the grace before anything is
    /// said starts now.
    fn close_turn(
        &mut self,
        now: u64,
        number: usize,
        text: Option<String>,
        merged: bool,
        out: &mut Vec<Effect>,
    ) {
        let Some(turn) = self.turns.remove(&number) else {
            return;
        };
        let event = match text {
            Some(text) => {
                let ended = turn.ended_ms.unwrap_or(now);
                TurnEvent::Finished {
                    turn_id: turn.id,
                    language: self.config.language.clone(),
                    text,
                    started_at: self.epoch_unix_ms + turn.started_ms,
                    ended_at: self.epoch_unix_ms + ended,
                    merged,
                    timings: TurnTimings {
                        audio_ms: turn.audio_ms,
                        endpoint_silence_ms: turn.silence_ms,
                        recognition_ms: turn.recognised_ms.unwrap_or(now).saturating_sub(ended),
                    },
                }
            }
            None => TurnEvent::Cancelled {
                turn_id: turn.id,
                merged,
            },
        };
        out.push(Effect::Event(VoiceEvent::Turn(event)));
        self.quiet_until = now + u64::from(self.config.audio_grace_ms);
    }

    /// Turns the playback's actions into effects.
    fn act(&mut self, actions: Vec<Action>, out: &mut Vec<Effect>) {
        for action in actions {
            out.push(match action {
                Action::Synthesize {
                    utterance,
                    chunk,
                    text,
                    language,
                } => Effect::Synthesize {
                    utterance,
                    chunk,
                    text,
                    language,
                },
                Action::Play {
                    utterance,
                    chunk,
                    samples,
                    sample_rate,
                } => Effect::Play {
                    utterance,
                    chunk,
                    samples,
                    sample_rate,
                },
                Action::Stop => Effect::StopPlayback,
                Action::Say { utterance, event } => Effect::Say {
                    id: utterance,
                    event,
                },
                Action::Error(code) => Effect::Event(VoiceEvent::Error(VoiceError { code })),
            });
        }
    }

    fn error(&self, code: &str, out: &mut Vec<Effect>) {
        out.push(Effect::Event(VoiceEvent::Error(VoiceError {
            code: code.to_owned(),
        })));
    }
}

/// The numbers segmentation takes from a configuration.
fn segmentation(config: &VoiceConfig) -> Segmentation {
    let (pause, longest) = config.patience.smart_turn_ms();
    let smart = config.end_of_turn == EndOfTurn::SmartTurn;
    Segmentation {
        end_of_turn_silence_ms: if smart {
            longest
        } else {
            config.patience.end_of_turn_silence_ms()
        },
        pause_ms: smart.then_some(pause),
        quiet_bar: config.listening_bar.quiet,
        playing_bar: config.listening_bar.playing,
    }
}
