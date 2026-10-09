//! The playback queue: replies wait in order, the first is spoken chunk by chunk (each synthesized while the one
//! before plays) and played, and its heard position moves as each chunk finishes playing at the speaker. A barge-in
//! stops the reply that plays and drops every reply queued behind it; a reply that arrives again under an id already
//! taken is ignored, unless it is a replay the person asked for.
//!
//! The heard position is counted at chunk boundaries: a chunk counts as heard once its last sample left the
//! speaker, never in part, since nothing here knows when each word sounded.

#[cfg(test)]
mod tests;

use std::collections::{HashSet, VecDeque};

use crate::event::PlaybackState;
use crate::room::{PlaybackStatus, Reply};
use crate::speech::{chunks, Chunk};

/// A reply in the queue.
#[derive(Debug)]
pub(crate) struct Utterance {
    pub(crate) id: String,
    pub(crate) language: Option<String>,
    chars: usize,
    chunks: Vec<Chunk>,
}

/// The reply being spoken.
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
    /// Whether `playing` was reported.
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
    /// What became of a reply.
    Status {
        utterance: String,
        status: PlaybackStatus,
        heard_chars: usize,
        reason: Option<String>,
    },
    /// Where the reader is: the chunk sounding (its characters) and what was heard before it.
    Position {
        utterance: String,
        chunk: Option<std::ops::Range<usize>>,
        heard_chars: usize,
    },
}

/// The queue and the reply being spoken.
#[derive(Debug, Default)]
pub(crate) struct Playback {
    queue: VecDeque<Utterance>,
    current: Option<Current>,
    seen: HashSet<String>,
}

impl Playback {
    /// Queues a reply. A reply with nothing to say is heard at once.
    pub(crate) fn push(&mut self, reply: Reply, actions: &mut Vec<Action>) {
        let fresh = self.seen.insert(reply.utterance_id.clone());
        if !fresh && !reply.replay {
            return;
        }
        let chunks = chunks(&reply.text);
        if chunks.is_empty() {
            actions.push(status(&reply.utterance_id, PlaybackStatus::Heard, 0));
            return;
        }
        self.queue.push_back(Utterance {
            id: reply.utterance_id,
            language: reply.language,
            chars: reply.text.chars().count(),
            chunks,
        });
    }

    /// Whether a reply is being spoken.
    pub(crate) fn busy(&self) -> bool {
        self.current.is_some()
    }

    /// Whether sound is coming out.
    pub(crate) fn sounding(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|current| current.sounding.is_some())
    }

    /// Whether any reply waits.
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

    /// Starts the next reply, if none is being spoken.
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
            Err(reason) => {
                let current = self.current.take().expect("the current reply");
                if current.sounding.is_some() {
                    actions.push(Action::Stop);
                }
                actions.push(Action::Status {
                    utterance: current.utterance.id.clone(),
                    status: PlaybackStatus::Failed,
                    heard_chars: heard(&current),
                    reason: Some(reason),
                });
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
            actions.push(status(utterance, PlaybackStatus::Playing, heard(current)));
        }
        actions.push(Action::Position {
            utterance: utterance.to_owned(),
            chunk: Some(current.utterance.chunks[chunk].chars.clone()),
            heard_chars: heard(current),
        });
    }

    /// The output played a chunk to its last sample. The last chunk makes the reply heard.
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
            actions.push(Action::Position {
                utterance: utterance.to_owned(),
                chunk: None,
                heard_chars: heard(current),
            });
            return;
        }
        let current = self.current.take().expect("the current reply");
        let chars = current.utterance.chars;
        actions.push(Action::Position {
            utterance: utterance.to_owned(),
            chunk: None,
            heard_chars: chars,
        });
        actions.push(status(utterance, PlaybackStatus::Heard, chars));
    }

    /// The person spoke over the playback, or the call stopped: the reply being spoken ends (interrupted if it had
    /// sounded, unplayed if not) and every queued one is dropped as unplayed.
    pub(crate) fn interrupt(&mut self, actions: &mut Vec<Action>) {
        if let Some(current) = self.current.take() {
            actions.push(Action::Stop);
            let state = if current.started {
                PlaybackStatus::Interrupted
            } else {
                PlaybackStatus::Unplayed
            };
            actions.push(status(&current.utterance.id, state, heard(&current)));
        }
        for utterance in self.queue.drain(..) {
            actions.push(status(&utterance.id, PlaybackStatus::Unplayed, 0));
        }
    }

    /// The current reply if it is `utterance`.
    fn current_for(&mut self, utterance: &str) -> Option<&mut Current> {
        self.current
            .as_mut()
            .filter(|current| current.utterance.id == utterance)
    }

    fn synthesize_next(&mut self, actions: &mut Vec<Action>) {
        let Some(current) = &mut self.current else {
            return;
        };
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

/// What was heard of the current reply: up to the end of the last chunk played to its end.
fn heard(current: &Current) -> usize {
    current
        .played
        .checked_sub(1)
        .map_or(0, |last| current.utterance.chunks[last].chars.end)
}

fn status(utterance: &str, status: PlaybackStatus, heard_chars: usize) -> Action {
    Action::Status {
        utterance: utterance.to_owned(),
        status,
        heard_chars,
        reason: None,
    }
}
