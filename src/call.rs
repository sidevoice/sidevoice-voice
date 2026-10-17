//! The call as a pure state machine: three regions in parallel, driven by [`Input`]s and a monotonic time in
//! milliseconds, answering with [`Effect`]s. It holds no socket, no clock and no thread: whoever drives it runs the
//! models and the speaker, and feeds back what they did.
//!
//! - **Listening** (`turns`): `Idle → Listening ⇄ Speaking`: `Speaking` while the person speaks in an open turn, back
//!   to `Listening` when they pause in it and when it ends. A turn's audio goes to recognition.
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
    CallState, CaptureHearing, Listening, Microphone, PlaybackCapture, SayCapture, TurnEvent,
    TurnTimings, VoiceError, VoiceEvent,
};
use crate::playback::{Action, Playback};
use crate::recognition::{accepted, Job, Outcome, Pending, Recognition};
use crate::say::{SayEvent, SayOutcome, StopReason};
use crate::take::{TakePause, TakeTurn};
use crate::turns::{duration_ms, Segment, Segmentation, Segmenter, SILENCE_STOP_MS};

/// What happens to the call.
#[derive(Debug)]
pub(crate) enum Input {
    /// Start listening; with `take`, keep where the turns fall in what is heard from now ([`Call::take_marks`]).
    Start { take: bool },
    /// Stop: the open turn and the turns not yet transcribed are cancelled, and what is being said stops.
    Stop,
    /// A new configuration.
    Config(Box<VoiceConfig>),
    /// Opaque monotonic host marker; the next capture copies the latest processed value.
    EvidenceMark(u64),
    /// Host publication observed on the browser AudioContext graph clock.
    EvidenceMarkAt(u64, u64),
    /// One detector window: 16 kHz mono samples after echo cancellation, whether the detector holds it for speech, and
    /// its probability that the window is speech. The probability decides; a detector that gives none is taken at its
    /// word, window by window.
    Window {
        pcm: Vec<f32>,
        speech: bool,
        probability: Option<f32>,
        /// Source-owned speaker evidence before these samples entered async VAD.
        evidence: Option<PlaybackCapture>,
        /// Unix time of the first captured sample; absent in direct state-machine drives.
        captured_at: Option<u64>,
    },
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
        evidence_key: Option<String>,
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
    #[cfg(any(web, test))]
    ChunkPlayedAt {
        utterance: String,
        chunk: usize,
        frame: u64,
    },
    /// The output stream has ended after its last rendered sample.
    PlaybackEnded,
    /// Whether the microphone is muted: muted windows are silence, and the open turn ends as ordinary silence ends it.
    Mute(bool),
    /// Whether the microphone's device gives no audio for now (not the person's mute).
    MicrophoneMuted(bool),
    /// The person cancels what they said that is not reported yet.
    Cancel,
    /// Close the open turn now with what was captured so far; nothing when none is open.
    EndTurn,
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

/// The end-of-turn probability above which a paused turn is over.
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
    /// Its marks in the take being kept, if one is: their index in it.
    mark: Option<usize>,
}

#[derive(Debug)]
struct HeardSay {
    id: String,
    capture_id: String,
    /// Latest ordered host publication when this say entered the actor. A
    /// later mark's graph frame proves this say did not exist at an earlier
    /// microphone cutoff, even if VAD delivers the callback afterwards.
    observed_mark: u64,
    chars: usize,
    confirmed: usize,
    playing: bool,
    complete: bool,
    played_order: u64,
    completed_frames: Vec<(u64, usize)>,
}

impl HeardSay {
    fn capture(&self) -> SayCapture {
        let (hearing, heard_chars) = if self.complete {
            (CaptureHearing::Complete, Some(self.chars))
        } else if self.confirmed > 0 {
            (CaptureHearing::Partial, Some(self.confirmed))
        } else if self.playing {
            (CaptureHearing::Unknown, None)
        } else {
            (CaptureHearing::NotPlayed, Some(0))
        };
        SayCapture {
            say_id: self.capture_id.clone(),
            hearing,
            heard_chars,
            played_order: (self.played_order > 0).then_some(self.played_order),
        }
    }
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
    heard_says: VecDeque<HeardSay>,
    played_order: u64,
    evidence_mark: u64,
    timed_marks: VecDeque<(u64, u64)>,
    source_timeline_overflow: bool,
    next_candidate: u64,
    candidate_id: Option<String>,
    candidate_onset_at: Option<u64>,
    /// A chunk ended while its utterance still has audio; the next chunk may be contiguous.
    continuing_chunk: Option<String>,
    /// Nothing starts to be said before this time (the grace after a turn).
    quiet_until: u64,
    /// Ids cancelled before the call heard of them: said later, they never play.
    cancelled: VecDeque<String>,
    /// Samples heard in detector windows since the call started: where the next window falls in a take.
    heard: usize,
    /// The marks of the turns in the take being kept, while one is.
    marks: Option<Vec<TakeTurn>>,
    /// Where the open turn's speech stopped, while it is in a pause, for the take being kept.
    pause_from: Option<usize>,
    state: Option<CallState>,
}

impl Call {
    /// Capture speaker state at microphone ingress, before the VAD can delay its callback.
    pub(crate) fn playback_capture(&self) -> PlaybackCapture {
        const LIMIT: usize = 64;
        let recent = self
            .heard_says
            .iter()
            .rev()
            .take(LIMIT)
            .map(HeardSay::capture)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let last_played = self
            .heard_says
            .iter()
            .filter(|say| say.played_order > 0)
            .max_by_key(|say| say.played_order)
            .map(HeardSay::capture);
        PlaybackCapture {
            available: true,
            mark: self.evidence_mark,
            recent,
            last_played,
            truncated: self.heard_says.len() > LIMIT,
        }
    }

    /// What actual, completed browser output can prove at a microphone graph frame.
    /// This clock is AudioContext processing time, not calibrated hardware time.
    pub(crate) fn playback_capture_at(&self, frame: u64) -> PlaybackCapture {
        const LIMIT: usize = 64;
        if self.source_timeline_overflow {
            return PlaybackCapture::unknown();
        }
        let cutoff_mark = self
            .timed_marks
            .iter()
            .filter(|(at, _)| *at <= frame)
            .map(|(_, mark)| *mark)
            .max()
            .unwrap_or(0);
        let capture = |say: &HeardSay| {
            let confirmed = say
                .completed_frames
                .iter()
                .filter(|(at, _)| *at <= frame)
                .map(|(_, chars)| *chars)
                .max()
                .unwrap_or(0);
            let last = say
                .completed_frames
                .iter()
                .filter(|(at, _)| *at <= frame)
                .map(|(at, _)| *at)
                .max();
            let hearing = if confirmed >= say.chars && say.chars > 0 {
                CaptureHearing::Complete
            } else if confirmed > 0 {
                CaptureHearing::Partial
            } else {
                CaptureHearing::Unknown
            };
            SayCapture {
                say_id: say.capture_id.clone(),
                hearing,
                heard_chars: (confirmed > 0).then_some(confirmed),
                played_order: last,
            }
        };
        let recent = self
            .heard_says
            .iter()
            .filter(|say| say.observed_mark <= cutoff_mark)
            .rev()
            .take(LIMIT)
            .map(&capture)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let all = self
            .heard_says
            .iter()
            .filter(|say| say.observed_mark <= cutoff_mark)
            .map(&capture)
            .collect::<Vec<_>>();
        let uncertain = all
            .iter()
            .any(|say| say.hearing != CaptureHearing::Complete);
        let last_played = if uncertain {
            None
        } else {
            all.into_iter()
                .filter(|say| say.played_order.is_some())
                .max_by_key(|say| say.played_order)
        };
        PlaybackCapture {
            available: !uncertain,
            mark: cutoff_mark,
            recent,
            last_played,
            truncated: self
                .heard_says
                .iter()
                .filter(|say| say.observed_mark <= cutoff_mark)
                .count()
                > LIMIT,
        }
    }

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
            heard_says: VecDeque::new(),
            played_order: 0,
            timed_marks: VecDeque::new(),
            source_timeline_overflow: false,
            evidence_mark: 0,
            next_candidate: 0,
            candidate_id: None,
            candidate_onset_at: None,
            continuing_chunk: None,
            quiet_until: 0,
            cancelled: VecDeque::new(),
            state: None,
            heard: 0,
            marks: None,
            pause_from: None,
        }
    }

    /// Takes one input at `now` (the call's monotonic milliseconds) and returns what to do.
    pub(crate) fn handle(&mut self, now: u64, input: Input) -> Vec<Effect> {
        let mut out = Vec::new();
        match input {
            Input::Start { take } => {
                self.started = true;
                self.heard = 0;
                self.marks = take.then(Vec::new);
                self.timed_marks.clear();
                // A pruned source identity or timeline cannot be reconstructed
                // by restarting the same call, even if its graph clock resets.
                for say in &mut self.heard_says {
                    say.completed_frames.clear()
                }
            }
            Input::Stop => self.stop(now, &mut out),
            Input::Config(config) => {
                self.segmenter.set_numbers(segmentation(&config));
                self.config = *config;
            }
            Input::EvidenceMark(mark) => {
                self.evidence_mark = self.evidence_mark.max(mark);
            }
            Input::EvidenceMarkAt(mark, frame) => {
                self.evidence_mark = self.evidence_mark.max(mark);
                self.timed_marks.push_back((frame, mark));
                if self.timed_marks.len() > 2048 {
                    self.source_timeline_overflow = true;
                    self.timed_marks.clear()
                }
            }
            Input::Window {
                pcm,
                speech,
                probability,
                evidence,
                captured_at,
            } => {
                let confidence = probability.unwrap_or(if speech { 1.0 } else { 0.0 });
                self.window(now, &pcm, confidence, evidence, captured_at, &mut out);
            }
            Input::Transcribed { turn, result } => self.transcribed(now, turn, result, &mut out),
            Input::Say {
                id,
                text,
                language,
                evidence_key,
            } => self.say(id, &text, language, evidence_key, &mut out),
            Input::CancelSay(id) => {
                let was_sounding = self.playback.sounding();
                let mut actions = Vec::new();
                if !self.playback.cancel(&id, &mut actions) {
                    // Not here (yet): a cancel may overtake its own `say`, which then never plays.
                    self.cancelled.push_back(id);
                    if self.cancelled.len() > CANCELS_KEPT {
                        self.cancelled.pop_front();
                    }
                }
                self.act(actions, &mut out);
                self.playback_boundary(was_sounding, &mut out);
            }
            Input::Synthesized {
                utterance,
                chunk,
                result,
            } => {
                let was_sounding = self.playback.sounding();
                let mut actions = Vec::new();
                self.playback
                    .synthesized(&utterance, chunk, result, &mut actions);
                self.act(actions, &mut out);
                self.playback_boundary(was_sounding, &mut out);
            }
            Input::ChunkStarted { utterance, chunk } => {
                let was_sounding = self.playback.sounding();
                let continuous =
                    self.continuing_chunk.take().as_deref() == Some(utterance.as_str());
                let mut actions = Vec::new();
                self.playback.chunk_started(&utterance, chunk, &mut actions);
                self.act(actions, &mut out);
                if !continuous {
                    self.playback_boundary(was_sounding, &mut out);
                }
            }
            Input::ChunkPlayed { utterance, chunk } => {
                let was_sounding = self.playback.sounding();
                let mut actions = Vec::new();
                self.playback.chunk_played(&utterance, chunk, &mut actions);
                self.act(actions, &mut out);
                if was_sounding && !self.playback.sounding() && self.playback.busy() {
                    self.continuing_chunk = Some(utterance);
                } else {
                    self.playback_boundary(was_sounding, &mut out);
                }
            }
            #[cfg(any(web, test))]
            Input::ChunkPlayedAt {
                utterance,
                chunk,
                frame,
            } => {
                let was_sounding = self.playback.sounding();
                let mut actions = Vec::new();
                self.playback.chunk_played(&utterance, chunk, &mut actions);
                self.act(actions, &mut out);
                if let Some(say) = self
                    .heard_says
                    .iter_mut()
                    .rev()
                    .find(|say| say.id == utterance)
                {
                    if say.completed_frames.len() < 256 {
                        say.completed_frames.push((frame, say.confirmed))
                    } else {
                        self.source_timeline_overflow = true
                    }
                }
                if was_sounding && !self.playback.sounding() && self.playback.busy() {
                    self.continuing_chunk = Some(utterance);
                } else {
                    self.playback_boundary(was_sounding, &mut out);
                }
            }
            Input::PlaybackEnded => {
                self.continuing_chunk = None;
                self.segmenter.reset_candidate();
            }
            Input::Mute(muted) => {
                self.muted = muted;
                if muted {
                    self.cancel_candidate(&mut out);
                    self.segmenter.reset_candidate();
                }
            }
            Input::MicrophoneMuted(muted) => self.microphone_muted = muted,
            Input::Cancel => self.cancel_input(now, &mut out),
            Input::EndTurn => {
                if let Some(segment) = self.segmenter.end_now() {
                    self.segment(now, segment, &mut out);
                }
            }
            Input::EndOfTurn {
                turn,
                pause,
                result,
            } => match result {
                Ok(probability) if probability > END_OF_TURN_LIKELY && self.open == Some(turn) => {
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

    /// The marks of the take being kept since the call started, which it stops keeping: where each turn's speech
    /// started, paused and was ended, in what was heard. Empty when the start asked for no take.
    pub(crate) fn take_marks(&mut self) -> Vec<TakeTurn> {
        self.marks.take().unwrap_or_default()
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

    /// One detector window: counted in what was heard whatever becomes of it, so that a take's positions follow its
    /// audio, muted windows included.
    fn window(
        &mut self,
        now: u64,
        pcm: &[f32],
        confidence: f32,
        evidence: Option<PlaybackCapture>,
        captured_at: Option<u64>,
        out: &mut Vec<Effect>,
    ) {
        self.heard += pcm.len();
        if !self.started || self.muted {
            if self.started && self.muted {
                self.window_silence(now, pcm.len(), out);
            }
            return;
        }
        let playing = self.playback.sounding();
        let paused = self.segmenter.paused();
        let segment = self.segmenter.window(now, pcm, confidence, playing);
        let (candidate_started, candidate_cancelled) = self.segmenter.candidate_transition();
        if candidate_cancelled {
            self.cancel_candidate(out);
        }
        if candidate_started && self.candidate_id.is_none() {
            let id = format!("{}-candidate-{}", self.call_id, self.next_candidate);
            self.next_candidate += 1;
            self.candidate_id = Some(id.clone());
            let onset_at = captured_at
                .unwrap_or(self.epoch_unix_ms + now.saturating_sub(duration_ms(pcm.len())));
            self.candidate_onset_at = Some(onset_at);
            out.push(Effect::Event(VoiceEvent::Turn(TurnEvent::Candidate {
                candidate_id: id,
                onset_at,
                playback: evidence.unwrap_or_else(PlaybackCapture::unknown),
            })));
        }
        if segment.is_none() && !self.segmenter.candidate_active() {
            self.cancel_candidate(out);
        }
        self.mark_pause(paused);
        if let Some(segment) = segment {
            self.segment(now, segment, out);
        }
        out.push(Effect::Event(VoiceEvent::Level(self.segmenter.level())));
    }

    fn window_silence(&mut self, now: u64, samples: usize, out: &mut Vec<Effect>) {
        let silence = vec![0.0; samples];
        let paused = self.segmenter.paused();
        if let Some(segment) = self
            .segmenter
            .window(now, &silence, 0.0, self.playback.sounding())
        {
            self.segment(now, segment, out);
        }
        if !self.segmenter.candidate_active() {
            self.cancel_candidate(out);
        }
        self.mark_pause(paused);
    }

    fn cancel_candidate(&mut self, out: &mut Vec<Effect>) {
        self.candidate_onset_at = None;
        if let Some(candidate_id) = self.candidate_id.take() {
            out.push(Effect::Event(VoiceEvent::Turn(
                TurnEvent::CandidateCancelled { candidate_id },
            )));
        }
    }

    /// What a segment does to the turns, where what was heard so far ends (a window's, the end of that window).
    fn segment(&mut self, now: u64, segment: Segment, out: &mut Vec<Effect>) {
        match segment {
            Segment::Started => {
                let number = self.next_turn;
                self.next_turn += 1;
                let id = format!("{}-turn-{number}", self.call_id);
                // Its speech, in the take being kept: where the speech that started it began.
                let speech = self.heard.saturating_sub(self.segmenter.start_run());
                self.pause_from = None;
                let mark = self.marks.as_mut().map(|marks| {
                    marks.push(TakeTurn {
                        turn_id: id.clone(),
                        speech_ms: duration_ms(speech),
                        pauses: Vec::new(),
                        end_ms: None,
                    });
                    marks.len() - 1
                });
                self.turns.insert(
                    number,
                    Turn {
                        id: id.clone(),
                        started_ms: self
                            .candidate_onset_at
                            .unwrap_or(self.epoch_unix_ms + now)
                            .saturating_sub(self.epoch_unix_ms),
                        ended_ms: None,
                        audio_ms: 0,
                        silence_ms: 0,
                        recognised_ms: None,
                        mark,
                    },
                );
                self.open = Some(number);
                out.push(Effect::Event(VoiceEvent::Turn(TurnEvent::Started {
                    turn_id: id,
                    candidate_id: self.candidate_id.take(),
                    started_at: self
                        .candidate_onset_at
                        .take()
                        .unwrap_or(self.epoch_unix_ms + now),
                })));
                let mut actions = Vec::new();
                self.playback.interrupt(StopReason::BargeIn, &mut actions);
                self.continuing_chunk = None;
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
                // Decided here: what was heard so far.
                let heard = duration_ms(self.heard);
                if let Some(mark) = self.mark(number) {
                    mark.end_ms = Some(heard);
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

    /// The marks of turn `number` in the take being kept, if one is.
    fn mark(&mut self, number: usize) -> Option<&mut TakeTurn> {
        let index = self.turns.get(&number)?.mark?;
        self.marks.as_mut()?.get_mut(index)
    }

    /// The open turn's pauses in the take being kept, after a window (`was`: whether it was in a pause before it):
    /// where its speech stopped, once the detector says it did, and where it began again, once it has started again.
    /// A pause the turn ends in is not one of them.
    fn mark_pause(&mut self, was: bool) {
        if self.marks.is_none() {
            return;
        }
        let paused = self.segmenter.paused();
        if !was && paused {
            self.pause_from = Some(self.heard.saturating_sub(self.segmenter.silence()));
        } else if was && !paused && self.segmenter.speaking() {
            let to = self.heard.saturating_sub(self.segmenter.start_run());
            let from = self.pause_from.take();
            if let (Some(from), Some(mark)) = (from, self.open.and_then(|number| self.mark(number)))
            {
                mark.pauses.push(TakePause {
                    from_ms: duration_ms(from),
                    to_ms: duration_ms(to),
                });
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
        self.cancel_candidate(out);
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

    fn finish_pending(&mut self, now: u64, pending: Pending, out: &mut Vec<Effect>) {
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

    fn stop(&mut self, now: u64, out: &mut Vec<Effect>) {
        self.cancel_candidate(out);
        self.cancel_input(now, out);
        // The next start hears a new stream: nothing of this one's volume or speech carries over.
        self.segmenter = Segmenter::new(segmentation(&self.config));
        let mut actions = Vec::new();
        self.playback.interrupt(StopReason::Stopped, &mut actions);
        self.continuing_chunk = None;
        self.act(actions, out);
        self.started = false;
        self.microphone_muted = false;
        self.marks = None;
        self.pause_from = None;
        // A stop is always answered with the state, even an unchanged one: the host knows it took effect.
        self.state = None;
    }

    /// Something to say: queued, or not played at all while the call is stopped.
    fn say(
        &mut self,
        id: String,
        text: &str,
        language: Option<String>,
        evidence_key: Option<String>,
        out: &mut Vec<Effect>,
    ) {
        self.heard_says.push_back(HeardSay {
            id: id.clone(),
            chars: text.chars().count(),
            confirmed: 0,
            capture_id: evidence_key.unwrap_or_else(|| id.clone()),
            observed_mark: self.evidence_mark,
            playing: false,
            complete: false,
            played_order: 0,
            completed_frames: Vec::new(),
        });
        if self.heard_says.len() > 256 {
            self.heard_says.pop_front();
            self.source_timeline_overflow = true;
        }
        let refused =
            if let Some(index) = self.cancelled.iter().position(|cancelled| *cancelled == id) {
                self.cancelled.remove(index);
                Some(StopReason::Cancelled)
            } else {
                if !self.started {
                    Some(StopReason::Stopped)
                } else if !self.quiet(self.segmenter.speaking()) {
                    Some(StopReason::BargeIn)
                } else {
                    None
                }
            };
        if let Some(reason) = refused {
            self.act(
                vec![Action::Say {
                    utterance: id,
                    event: SayEvent::Done {
                        outcome: SayOutcome::NotPlayed { reason },
                    },
                }],
                out,
            );
            return;
        }
        let language = language.or_else(|| self.config.language.clone());
        let mut actions = Vec::new();
        self.playback.push(id, text, language, &mut actions);
        self.act(actions, out);
    }

    fn playback_boundary(&mut self, was_sounding: bool, _out: &mut Vec<Effect>) {
        if was_sounding != self.playback.sounding()
            || (!self.playback.busy() && self.continuing_chunk.take().is_some())
        {
            self.segmenter.reset_candidate();
        }
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
            self.finish_pending(now, pending, out);
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
            } else if speaking && !self.segmenter.paused() {
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
            let effect = match action {
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
                Action::Say { utterance, event } => {
                    if let Some(say) = self
                        .heard_says
                        .iter_mut()
                        .rev()
                        .find(|say| say.id == utterance)
                    {
                        match &event {
                            SayEvent::Playing => {
                                say.playing = true;
                                self.played_order += 1;
                                say.played_order = self.played_order;
                            }
                            SayEvent::Progress { heard_chars, .. } => {
                                say.confirmed = say.confirmed.max(*heard_chars);
                            }
                            SayEvent::Done { outcome } => match outcome {
                                SayOutcome::Heard => {
                                    say.complete = true;
                                    say.confirmed = say.chars;
                                }
                                SayOutcome::HeardUpTo { heard_chars, .. } => {
                                    say.confirmed = say.confirmed.max(*heard_chars);
                                }
                                SayOutcome::NotPlayed { .. } => {}
                            },
                        }
                    }
                    Effect::Say {
                        id: utterance,
                        event,
                    }
                }
                Action::Error(code) => Effect::Event(VoiceEvent::Error(VoiceError { code })),
            };
            out.push(effect);
        }
    }

    fn error(&self, code: &str, out: &mut Vec<Effect>) {
        out.push(Effect::Event(VoiceEvent::Error(VoiceError {
            code: code.to_owned(),
        })));
    }
}

/// The numbers segmentation takes from a configuration: with `smart-turn`, the patience's stop and longest pause;
/// with `silence`, a short stop and the patience's silence after it.
fn segmentation(config: &VoiceConfig) -> Segmentation {
    let patience = config.patience;
    let smart = config.end_of_turn == EndOfTurn::SmartTurn;
    Segmentation {
        stop_ms: if smart {
            patience.smart_turn_stop_ms()
        } else {
            SILENCE_STOP_MS
        },
        end_ms: if smart {
            patience.smart_turn_longest_ms()
        } else {
            patience.silence_ms()
        },
        ask: smart,
        quiet_bar: config.listening_bar.quiet,
        playing_bar: config.listening_bar.playing,
        barge_in_min_ms: (config.barge_in_min_speech.seconds() * 1_000.0).ceil() as u32,
    }
}
