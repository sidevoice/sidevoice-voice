//! The playback queue: what the call is asked to say waits in order; the first is spoken chunk by chunk (each
//! synthesized while the one before plays, never further ahead: at most [`AHEAD`] chunks are at the output unplayed) and
//! played, and its heard position moves as each chunk finishes playing at the speaker. A barge-in, a stop or a cancel
//! ends what it reaches, and each ending says how ([`SayOutcome`]).
//!
//! The heard position is counted at chunk boundaries: a chunk counts as heard once its last sample left the speaker,
//! never in part, since nothing here knows when each word sounded.

#[cfg(test)]
mod tests;

use std::collections::VecDeque;

use crate::event::PlaybackState;
use crate::say::{SayEvent, SayOutcome, StopReason};
use crate::speech::{chunks, Chunk};

/// How many chunks may be synthesized and not yet played: the one at the output and the next.
pub(crate) const AHEAD: usize = 2;

/// Something to say, in the queue.
#[derive(Debug)]
pub(crate) struct Utterance {
    pub(crate) id: String,
    pub(crate) language: Option<String>,
    chars: usize,
    chunks: Vec<Chunk>,
}

/// What is being said.
#[derive(Debug)]
struct Current {
    utterance: Utterance,
    /// The next chunk to synthesize, and whether its synthesis is under way.
    next: usize,
    synthesizing: bool,
    /// The chunk sounding now, once one is.
    sounding: Option<usize>,
    /// Chunks played to their end.
    played: usize,
    /// Whether it sounded.
    started: bool,
}

/// What the playback asks for, and what it reports.
#[derive(Debug, PartialEq)]
pub(crate) enum Action {
    /// Speak this chunk.
    Synthesize {
        utterance: String,
        chunk: usize,
        text: String,
        language: Option<String>,
    },
    /// Play this chunk's audio after whatever plays now.
    Play {
        utterance: String,
        chunk: usize,
        samples: Vec<f32>,
        sample_rate: u32,
    },
    /// Stop the output now, with a short fade, and drop what it holds.
    Stop,
    /// A step of something being said.
    Say { utterance: String, event: SayEvent },
    /// Something could not be spoken: the stable code of why, for the host.
    Error(String),
}

/// The queue and what is being said.
#[derive(Debug, Default)]
pub(crate) struct Playback {
    queue: VecDeque<Utterance>,
    current: Option<Current>,
}

impl Playback {
    /// Queues `text` under `id`. Something with nothing to say is heard at once.
    pub(crate) fn push(
        &mut self,
        id: String,
        text: &str,
        language: Option<String>,
        actions: &mut Vec<Action>,
    ) {
        let chunks = chunks(text);
        if chunks.is_empty() {
            actions.push(done(&id, SayOutcome::Heard));
            return;
        }
        self.queue.push_back(Utterance {
            id,
            language,
            chars: text.chars().count(),
            chunks,
        });
    }

    /// Whether something is being said.
    pub(crate) fn busy(&self) -> bool {
        self.current.is_some()
    }

    /// Whether sound is coming out.
    pub(crate) fn sounding(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|current| current.sounding.is_some())
    }

    /// Whether anything waits.
    pub(crate) fn waiting(&self) -> bool {
        !self.queue.is_empty()
    }

    pub(crate) fn state(&self) -> PlaybackState {
        match &self.current {
            None => PlaybackState::Idle,
            Some(current) if current.sounding.is_some() => PlaybackState::Playing,
            Some(_) => PlaybackState::Synthesizing,
        }
    }

    /// Starts the next one, if nothing is being said.
    pub(crate) fn start(&mut self, actions: &mut Vec<Action>) {
        if self.current.is_some() {
            return;
        }
        let Some(utterance) = self.queue.pop_front() else {
            return;
        };
        self.current = Some(Current {
            utterance,
            next: 0,
            synthesizing: false,
            sounding: None,
            played: 0,
            started: false,
        });
        self.synthesize_next(actions);
    }

    /// Takes a chunk's speech: it goes to the output, and the next chunk is synthesized.
    pub(crate) fn synthesized(
        &mut self,
        utterance: &str,
        chunk: usize,
        result: Result<(Vec<f32>, u32), String>,
        actions: &mut Vec<Action>,
    ) {
        let Some(current) = self.current_for(utterance) else {
            return;
        };
        if !current.synthesizing || current.next != chunk {
            return;
        }
        current.synthesizing = false;
        match result {
            Ok((samples, sample_rate)) => {
                current.next += 1;
                actions.push(Action::Play {
                    utterance: utterance.to_owned(),
                    chunk,
                    samples,
                    sample_rate,
                });
                self.synthesize_next(actions);
            }
            Err(code) => {
                actions.push(Action::Error(code.clone()));
                self.end_current(StopReason::Failed { code }, actions);
            }
        }
    }

    /// The output started sounding a chunk.
    pub(crate) fn chunk_started(
        &mut self,
        utterance: &str,
        chunk: usize,
        actions: &mut Vec<Action>,
    ) {
        let Some(current) = self.current_for(utterance) else {
            return;
        };
        if chunk >= current.next || chunk < current.played {
            return;
        }
        current.sounding = Some(chunk);
        if !current.started {
            current.started = true;
            actions.push(say(utterance, SayEvent::Playing));
        }
        let range = current.utterance.chunks[chunk].chars.clone();
        let heard_chars = heard(current);
        actions.push(say(
            utterance,
            SayEvent::Progress {
                sounding: Some((range.start, range.end)),
                heard_chars,
            },
        ));
    }

    /// The output played a chunk to its last sample. The last chunk makes it heard.
    pub(crate) fn chunk_played(
        &mut self,
        utterance: &str,
        chunk: usize,
        actions: &mut Vec<Action>,
    ) {
        let Some(current) = self.current_for(utterance) else {
            return;
        };
        if chunk >= current.next || chunk < current.played {
            return;
        }
        current.played = chunk + 1;
        current.sounding = None;
        if current.played < current.utterance.chunks.len() {
            let heard_chars = heard(current);
            actions.push(say(
                utterance,
                SayEvent::Progress {
                    sounding: None,
                    heard_chars,
                },
            ));
            self.synthesize_next(actions);
            return;
        }
        let current = self.current.take().expect("what is being said");
        actions.push(say(
            utterance,
            SayEvent::Progress {
                sounding: None,
                heard_chars: current.utterance.chars,
            },
        ));
        actions.push(done(utterance, SayOutcome::Heard));
    }

    /// The person spoke over the playback (`BargeIn`), or the call stopped (`Stopped`): what is being said ends (cut
    /// if it had sounded, not played if not) and everything queued is dropped, not played, for the same reason.
    pub(crate) fn interrupt(&mut self, reason: StopReason, actions: &mut Vec<Action>) {
        self.end_current(reason.clone(), actions);
        for utterance in self.queue.drain(..) {
            actions.push(done(
                &utterance.id,
                SayOutcome::NotPlayed {
                    reason: reason.clone(),
                },
            ));
        }
    }

    /// Cancels `utterance`: being said, it stops; queued, it is dropped. Either way its outcome says `cancelled`.
    /// Answers whether it was here.
    pub(crate) fn cancel(&mut self, utterance: &str, actions: &mut Vec<Action>) -> bool {
        if self.current_for(utterance).is_some() {
            self.end_current(StopReason::Cancelled, actions);
            true
        } else if let Some(index) = self.queue.iter().position(|queued| queued.id == utterance) {
            self.queue.remove(index);
            actions.push(done(
                utterance,
                SayOutcome::NotPlayed {
                    reason: StopReason::Cancelled,
                },
            ));
            true
        } else {
            false
        }
    }

    /// Ends what is being said, for `reason`: what of it is at the output goes, and its outcome says how far it got.
    fn end_current(&mut self, reason: StopReason, actions: &mut Vec<Action>) {
        let Some(current) = self.current.take() else {
            return;
        };
        // Chunks handed to the output and not played to their end may sound yet: they go.
        if current.next > current.played || current.sounding.is_some() {
            actions.push(Action::Stop);
        }
        let outcome = if current.started {
            SayOutcome::HeardUpTo {
                heard_chars: heard(&current),
                reason,
            }
        } else {
            SayOutcome::NotPlayed { reason }
        };
        actions.push(done(&current.utterance.id, outcome));
    }

    /// The current one if it is `utterance`.
    fn current_for(&mut self, utterance: &str) -> Option<&mut Current> {
        self.current
            .as_mut()
            .filter(|current| current.utterance.id == utterance)
    }

    /// Synthesizes the next chunk, unless one is under way or [`AHEAD`] are at the output unplayed.
    fn synthesize_next(&mut self, actions: &mut Vec<Action>) {
        let Some(current) = &mut self.current else {
            return;
        };
        if current.synthesizing || current.next >= current.played + AHEAD {
            return;
        }
        let Some(chunk) = current.utterance.chunks.get(current.next) else {
            return;
        };
        current.synthesizing = true;
        actions.push(Action::Synthesize {
            utterance: current.utterance.id.clone(),
            chunk: current.next,
            text: chunk.text.clone(),
            language: current.utterance.language.clone(),
        });
    }
}

/// What was heard of what is being said: up to the end of the last chunk played to its end.
fn heard(current: &Current) -> usize {
    current
        .played
        .checked_sub(1)
        .map_or(0, |last| current.utterance.chunks[last].chars.end)
}

fn say(utterance: &str, event: SayEvent) -> Action {
    Action::Say {
        utterance: utterance.to_owned(),
        event,
    }
}

fn done(utterance: &str, outcome: SayOutcome) -> Action {
    say(utterance, SayEvent::Done { outcome })
}
