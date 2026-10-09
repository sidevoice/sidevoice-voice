//! The call as a pure state machine: three regions in parallel, driven by [`Input`]s and a monotonic time in
//! milliseconds, answering with [`Effect`]s. It holds no socket, no clock and no thread: whoever drives it runs the
//! models, the speaker and the room, and feeds back what they did.
//!
//! - **Listening** (`turns`): `Idle → Listening ⇄ Speaking`, and back to `Listening` when a turn ends. A turn's audio
//!   goes to recognition.
//! - **Recognition** (`recognition`): turns are transcribed one at a time, filtered, held for the merge window and
//!   reported as `finished`, `cancelled`, or joined into the next one.
//! - **Playback** (`playback`): `Idle → Synthesizing → Playing → Idle`, reply by reply. A turn that opens while a
//!   reply is on its way is a barge-in: the reply stops and every queued one is dropped. A reply waits while the
//!   person's turn is open or on its way to the room, and for the grace after it.

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::config::{EndOfTurn, VoiceConfig};
use crate::event::{CallState, Karaoke, Listening, VoiceError, VoiceEvent};
use crate::playback::{Action, Playback};
use crate::recognition::{accepted, Job, Outcome, Recognition};
use crate::room::{
    Playback as PlaybackReport, RoomEvent, RoomMessage, TurnPhase, TurnTimings, UserTurn,
};
use crate::turns::{duration_ms, Segment, Segmentation, Segmenter};

/// What happens to the call.
#[derive(Debug)]
pub(crate) enum Input {
    /// Start listening.
    Start,
    /// Stop: the open turn and the turns not yet transcribed are cancelled, and the playback is interrupted.
    Stop,
    /// A new configuration.
    Config(Box<VoiceConfig>),
    /// One detector window: 16 kHz mono samples after echo cancellation, and whether the detector holds it for speech.
    Window { pcm: Vec<f32>, speech: bool },
    /// A turn's transcript, or the stable code of why there is none.
    Transcribed {
        turn: usize,
        result: Result<Transcript, String>,
    },
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
    /// A message from the room.
    Room(RoomEvent),
    /// Whether the room is in reach.
    Online(bool),
    /// Whether the microphone is muted: an open turn ends with what was said.
    Mute(bool),
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

/// The end-of-turn probability from which a paused turn is over.
const END_OF_TURN_LIKELY: f32 = 0.5;

/// A transcript as the recogniser gave it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Transcript {
    pub(crate) text: String,
    /// The mean log-probability, where the recogniser reports one.
    pub(crate) logprob: Option<f64>,
}

/// What the driver must do.
#[derive(Debug, PartialEq)]
pub(crate) enum Effect {
    /// Tell the host.
    Event(VoiceEvent),
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

/// A turn the room was told of, until it is finished or cancelled.
#[derive(Debug)]
struct Turn {
    id: String,
    revision: u64,
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
    messages: u64,
    started: bool,
    muted: bool,
    online: bool,
    /// The latest revision seen on a reply.
    revision: u64,
    segmenter: Segmenter,
    /// The turn being spoken.
    open: Option<usize>,
    turns: HashMap<usize, Turn>,
    next_turn: usize,
    recognition: Recognition,
    playback: Playback,
    /// No reply starts before this time (the grace after a turn).
    quiet_until: u64,
    state: Option<CallState>,
}

impl Call {
    pub(crate) fn new(config: VoiceConfig, call_id: String, epoch_unix_ms: u64) -> Self {
        Self {
            segmenter: Segmenter::new(segmentation(&config)),
            config,
            call_id,
            epoch_unix_ms,
            messages: 0,
            started: false,
            muted: false,
            online: true,
            revision: 0,
            open: None,
            turns: HashMap::new(),
            next_turn: 0,
            recognition: Recognition::default(),
            playback: Playback::default(),
            quiet_until: 0,
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
            Input::Synthesized {
                utterance,
                chunk,
                result,
            } => {
                let mut actions = Vec::new();
                self.playback
                    .synthesized(&utterance, chunk, result, &mut actions);
                self.act(now, actions, &mut out);
            }
            Input::ChunkStarted { utterance, chunk } => {
                let mut actions = Vec::new();
                self.playback.chunk_started(&utterance, chunk, &mut actions);
                self.act(now, actions, &mut out);
            }
            Input::ChunkPlayed { utterance, chunk } => {
                let mut actions = Vec::new();
                self.playback.chunk_played(&utterance, chunk, &mut actions);
                self.act(now, actions, &mut out);
            }
            Input::Room(RoomEvent::Reply(reply)) => {
                self.revision = self.revision.max(reply.revision);
                let mut actions = Vec::new();
                self.playback.push(reply, &mut actions);
                self.act(now, actions, &mut out);
            }
            Input::Room(RoomEvent::Other) => {}
            Input::Online(online) => self.online = online,
            Input::Mute(muted) => {
                self.muted = muted;
                if muted {
                    if let Some(segment) = self.segmenter.close(0) {
                        self.segment(now, segment, &mut out);
                    }
                }
            }
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
        let reply = (self.playback.waiting() && !self.playback.busy() && self.quiet_until > 0)
            .then_some(self.quiet_until);
        [
            self.segmenter.deadline(),
            self.recognition.deadline(speaking),
            reply,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Does what is due at `now`: ends a turn whose audio stopped, reports a transcript whose merge window closed,
    /// starts a reply whose grace is over.
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
                self.turns.insert(
                    number,
                    Turn {
                        id: format!("{}-turn-{number}", self.call_id),
                        revision: self.revision,
                        started_ms: now,
                        ended_ms: None,
                        audio_ms: 0,
                        silence_ms: 0,
                        recognised_ms: None,
                    },
                );
                self.open = Some(number);
                self.report_turn(number, TurnPhase::Started, None, false, out);
                let mut actions = Vec::new();
                self.playback.interrupt(&mut actions);
                self.act(now, actions, out);
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
                        self.close_turn(now, number, TurnPhase::Cancelled, None, false, out);
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
        result: Result<Transcript, String>,
        out: &mut Vec<Effect>,
    ) {
        if !self.recognition.is_active(turn) {
            return;
        }
        let next = self.recognition.done();
        let language = self.config.language.clone();
        let text = match result {
            Ok(transcript) => accepted(&transcript.text, language.as_deref(), transcript.logprob),
            Err(code) => {
                self.error(&code, out);
                None
            }
        };
        match text {
            None => self.close_turn(now, turn, TurnPhase::Cancelled, None, false, out),
            Some(text) => {
                if let Some(record) = self.turns.get_mut(&turn) {
                    record.recognised_ms = Some(now);
                }
                let merge = u64::from(self.config.patience.merge_window_ms());
                if merge == 0 {
                    self.close_turn(now, turn, TurnPhase::Finished, Some(text), false, out);
                } else if let Outcome::Joined(joined) =
                    self.recognition.hold(turn, text, now + merge)
                {
                    for earlier in joined {
                        self.close_turn(now, earlier, TurnPhase::Cancelled, None, true, out);
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
            self.close_turn(now, turn, TurnPhase::Cancelled, None, false, out);
        }
    }

    fn stop(&mut self, now: u64, out: &mut Vec<Effect>) {
        self.cancel_input(now, out);
        let mut actions = Vec::new();
        self.playback.interrupt(&mut actions);
        self.act(now, actions, out);
        self.started = false;
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
            self.close_turn(
                now,
                *last,
                TurnPhase::Finished,
                Some(pending.text),
                merged,
                out,
            );
        }
        let quiet = !speaking && self.recognition.len() == 0 && !self.recognition.holding();
        if quiet && self.open.is_none() && now >= self.quiet_until && !self.playback.busy() {
            let mut actions = Vec::new();
            self.playback.start(&mut actions);
            self.act(now, actions, out);
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
            online: self.online,
        };
        if self.state != Some(state) {
            self.state = Some(state);
            out.push(Effect::Event(VoiceEvent::State(state)));
        }
    }

    /// Reports a turn's last phase and forgets it; the grace before the next reply starts now.
    fn close_turn(
        &mut self,
        now: u64,
        number: usize,
        phase: TurnPhase,
        text: Option<String>,
        merged: bool,
        out: &mut Vec<Effect>,
    ) {
        if !self.turns.contains_key(&number) {
            return;
        }
        self.report_turn(number, phase, text, merged, out);
        self.turns.remove(&number);
        self.quiet_until = now + u64::from(self.config.audio_grace_ms);
    }

    fn report_turn(
        &mut self,
        number: usize,
        phase: TurnPhase,
        text: Option<String>,
        merged: bool,
        out: &mut Vec<Effect>,
    ) {
        let Some(turn) = self.turns.get(&number) else {
            return;
        };
        let timings = turn.ended_ms.map(|ended| TurnTimings {
            audio_ms: turn.audio_ms,
            endpoint_silence_ms: turn.silence_ms,
            recognition_ms: turn
                .recognised_ms
                .map(|recognised| recognised.saturating_sub(ended)),
        });
        let message = UserTurn {
            client_msg_id: String::new(),
            turn_id: turn.id.clone(),
            phase,
            revision: turn.revision,
            language: text.as_ref().and(self.config.language.clone()),
            text,
            offline: !self.online,
            started_at: self.epoch_unix_ms + turn.started_ms,
            ended_at: turn.ended_ms.map(|ended| self.epoch_unix_ms + ended),
            merged,
            timings,
        };
        let message = UserTurn {
            client_msg_id: self.message_id(),
            ..message
        };
        out.push(Effect::Event(VoiceEvent::RoomMessage(
            RoomMessage::UserTurn(message),
        )));
    }

    /// Turns the playback's actions into effects and reports.
    fn act(&mut self, now: u64, actions: Vec<Action>, out: &mut Vec<Effect>) {
        for action in actions {
            match action {
                Action::Synthesize {
                    utterance,
                    chunk,
                    text,
                    language,
                } => out.push(Effect::Synthesize {
                    utterance,
                    chunk,
                    text,
                    language,
                }),
                Action::Play {
                    utterance,
                    chunk,
                    samples,
                    sample_rate,
                } => out.push(Effect::Play {
                    utterance,
                    chunk,
                    samples,
                    sample_rate,
                }),
                Action::Stop => out.push(Effect::StopPlayback),
                Action::Status {
                    utterance,
                    status,
                    heard_chars,
                    reason,
                } => {
                    if let Some(reason) = &reason {
                        self.error(reason, out);
                    }
                    let report = PlaybackReport {
                        client_msg_id: self.message_id(),
                        utterance_id: utterance,
                        status,
                        heard_chars,
                        reason,
                        at: self.epoch_unix_ms + now,
                    };
                    out.push(Effect::Event(VoiceEvent::RoomMessage(
                        RoomMessage::Playback(report),
                    )));
                }
                Action::Position {
                    utterance,
                    chunk,
                    heard_chars,
                } => out.push(Effect::Event(VoiceEvent::Karaoke(Karaoke {
                    utterance_id: utterance,
                    sounding: chunk.map(|range| (range.start, range.end)),
                    heard_chars,
                }))),
            }
        }
    }

    fn error(&self, code: &str, out: &mut Vec<Effect>) {
        out.push(Effect::Event(VoiceEvent::Error(VoiceError {
            code: code.to_owned(),
        })));
    }

    fn message_id(&mut self) -> String {
        self.messages += 1;
        format!("{}-{}", self.call_id, self.messages)
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
