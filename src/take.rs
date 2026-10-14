//! A take ([`Take`]): what the microphone gave a call from the moment it listened to the moment that listening ended,
//! as the call heard it, with where in it each of the person's turns started, paused and was ended. A call keeps one
//! only when its start asks for it, holds it in memory only, and hands it over as the listening ends, keeping no copy.
//!
//! Every time in a take is in milliseconds from its first sample, at the call's own positions in the audio: where the
//! detector's windows fall, not when anything was told. The audio holds at most two minutes: what comes after is not
//! kept, and neither are the marks past its end.

#[cfg(test)]
mod tests;

use serde::Serialize;

use crate::turns::{duration_ms, RATE};

/// The most audio one take keeps: two minutes.
const MAX_TAKE: usize = RATE as usize * 120;

/// What the microphone gave a call over one stretch of listening, and where the person's turns fell in it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Take {
    /// The samples' rate, in Hz: the call's own, 16 000.
    #[serde(rename = "sampleRate")]
    pub sample_rate: u32,
    /// Mono samples from −1 to 1, in order, as the call heard them (after echo cancellation); silence while the person
    /// had the microphone muted.
    pub samples: Vec<f32>,
    /// When its first sample was heard, and its end, in Unix milliseconds.
    pub started_at: u64,
    pub ended_at: u64,
    /// The turns the call opened in it, in order.
    pub turns: Vec<TakeTurn>,
}

/// One of the person's turns within a take, under the call's id for it (the `turn_id` of its [`TurnEvent`]s).
///
/// [`TurnEvent`]: crate::TurnEvent
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TakeTurn {
    pub turn_id: String,
    /// Where the call heard the speech the turn opened at.
    pub speech_ms: u64,
    /// Where its speech paused (a silence longer than the gaps between words) and where it came back, before its end.
    pub pauses: Vec<TakePause>,
    /// Where the call decided the turn had ended (its transcription starting); `None` when the take ended first, or
    /// the turn was cancelled while open.
    pub end_ms: Option<u64>,
}

/// A pause within a turn: from where its speech stopped to where it came back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TakePause {
    pub from_ms: u64,
    pub to_ms: u64,
}

/// The audio of a take being kept.
#[derive(Debug)]
pub(crate) struct Recording {
    started_at: u64,
    samples: Vec<f32>,
}

impl Recording {
    /// A take whose first sample is heard at `started_at`, in Unix milliseconds.
    pub(crate) fn new(started_at: u64) -> Self {
        Self {
            started_at,
            samples: Vec::new(),
        }
    }

    /// Keeps captured samples, or as much silence while the person has the microphone muted, up to the most a take
    /// keeps.
    pub(crate) fn extend(&mut self, pcm: &[f32], muted: bool) {
        let room = MAX_TAKE.saturating_sub(self.samples.len()).min(pcm.len());
        if muted {
            self.samples.resize(self.samples.len() + room, 0.0);
        } else {
            self.samples.extend_from_slice(&pcm[..room]);
        }
    }

    /// The take, with the marks of `turns` that fall within its audio: a turn that opened past its end is left out, a
    /// pause past its end too (one going on at its end stops there), and an end past it is unknown.
    pub(crate) fn finish(self, turns: Vec<TakeTurn>) -> Take {
        let length = duration_ms(self.samples.len());
        let turns = turns
            .into_iter()
            .filter(|turn| turn.speech_ms <= length)
            .map(|turn| TakeTurn {
                pauses: turn
                    .pauses
                    .into_iter()
                    .filter(|pause| pause.from_ms < length)
                    .map(|pause| TakePause {
                        from_ms: pause.from_ms,
                        to_ms: pause.to_ms.min(length),
                    })
                    .collect(),
                end_ms: turn.end_ms.filter(|&end| end <= length),
                ..turn
            })
            .collect();
        Take {
            sample_rate: RATE,
            samples: self.samples,
            started_at: self.started_at,
            ended_at: self.started_at + length,
            turns,
        }
    }
}
