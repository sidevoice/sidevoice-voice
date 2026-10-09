//! Segmentation: the detector's windows of 16 kHz audio become turns. A turn opens when a window is speech and loud
//! enough for the listening bar, with the second of audio before it (the pre-roll, which holds the speech the
//! detector needed to confirm); it ends after the patience's silence, when the audio stops arriving, or when it is
//! cancelled. A turn holds at most a minute of audio: what comes after is not kept.

mod level;

#[cfg(test)]
mod tests;

use std::collections::VecDeque;

#[cfg(test)]
pub(crate) use level::instant;
pub(crate) use level::Level;

/// The rate segmentation works at, in Hz.
pub(crate) const RATE: u32 = 16_000;
/// Samples per millisecond.
const PER_MS: u64 = RATE as u64 / 1000;
/// The audio kept before a turn opens.
const PRE_ROLL: usize = RATE as usize;
/// The most audio one turn keeps.
const MAX_TURN: usize = RATE as usize * 60;
/// How long an open turn waits for audio that stopped arriving (a muted or lost microphone) before it ends.
pub(crate) const AUDIO_IDLE_MS: u64 = 5_000;

/// The numbers segmentation runs on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Segmentation {
    /// Silence after the detector's end of speech that ends a turn.
    pub(crate) end_of_turn_silence_ms: u32,
    /// Silence after which the turn's audio is offered to an end-of-turn model, once per pause (`smart-turn`).
    pub(crate) pause_ms: Option<u32>,
    /// The level speech needs while nothing plays.
    pub(crate) quiet_bar: f32,
    /// The level speech needs to open a turn while a reply plays.
    pub(crate) playing_bar: f32,
}

/// What one window did to the turn.
#[derive(Debug, PartialEq)]
pub(crate) enum Segment {
    /// A turn opened.
    Started,
    /// The open turn ended, with its audio.
    Ended(EndedTurn),
    /// The open turn has paused for `pause_ms`: its audio so far, and the pause's number in the turn, to ask an
    /// end-of-turn model whether it is over.
    Paused { pcm: Vec<f32>, pause: u32 },
}

/// The audio of a turn that ended.
#[derive(Debug, PartialEq)]
pub(crate) struct EndedTurn {
    /// 16 kHz mono samples, the pre-roll included.
    pub(crate) pcm: Vec<f32>,
    /// The silence that ended it, in milliseconds.
    pub(crate) silence_ms: u64,
}

/// The turn being spoken.
#[derive(Debug)]
struct Open {
    pcm: Vec<f32>,
    /// Pauses so far, the one going on included.
    pauses: u32,
    /// Whether the pause going on was offered to an end-of-turn model.
    offered: bool,
    /// Samples of silence since the last speech.
    silent: u64,
    /// The time of the last window, in the call's milliseconds.
    last_audio_ms: u64,
}

/// Turns out of the detector's windows.
#[derive(Debug)]
pub(crate) struct Segmenter {
    numbers: Segmentation,
    level: Level,
    recent: VecDeque<f32>,
    open: Option<Open>,
}

impl Segmenter {
    pub(crate) fn new(numbers: Segmentation) -> Self {
        Self {
            numbers,
            level: Level::default(),
            recent: VecDeque::with_capacity(PRE_ROLL),
            open: None,
        }
    }

    pub(crate) fn set_numbers(&mut self, numbers: Segmentation) {
        self.numbers = numbers;
    }

    /// Whether a turn is open.
    pub(crate) fn speaking(&self) -> bool {
        self.open.is_some()
    }

    /// The smoothed level of the last window, from 0 to 1.
    pub(crate) fn level(&self) -> f32 {
        self.level.value()
    }

    /// Takes one detector window at `now`: its samples and whether the detector holds it for speech. `playing` says
    /// whether a reply is sounding, which raises the bar a turn must clear to open.
    pub(crate) fn window(
        &mut self,
        now: u64,
        pcm: &[f32],
        speech: bool,
        playing: bool,
    ) -> Option<Segment> {
        let level = self.level.take(pcm);
        match &mut self.open {
            None => {
                let bar = if playing {
                    self.numbers.playing_bar
                } else {
                    self.numbers.quiet_bar
                };
                if speech && level >= bar {
                    let mut audio: Vec<f32> = self.recent.drain(..).collect();
                    audio.extend_from_slice(pcm);
                    self.open = Some(Open {
                        pcm: audio,
                        pauses: 0,
                        offered: false,
                        silent: 0,
                        last_audio_ms: now,
                    });
                    return Some(Segment::Started);
                }
                self.recent.extend(pcm);
                let excess = self.recent.len().saturating_sub(PRE_ROLL);
                self.recent.drain(..excess);
                None
            }
            Some(open) => {
                open.last_audio_ms = now;
                let room = MAX_TURN.saturating_sub(open.pcm.len());
                open.pcm.extend_from_slice(&pcm[..pcm.len().min(room)]);
                if speech && level >= self.numbers.quiet_bar {
                    open.silent = 0;
                    open.offered = false;
                    return None;
                }
                if open.silent == 0 {
                    open.pauses += 1;
                }
                open.silent += pcm.len() as u64;
                let silence_ms = open.silent / PER_MS;
                if silence_ms >= u64::from(self.numbers.end_of_turn_silence_ms) {
                    return Some(self.end(silence_ms));
                }
                let pause = self
                    .numbers
                    .pause_ms
                    .is_some_and(|pause| silence_ms >= u64::from(pause));
                if pause && !open.offered {
                    open.offered = true;
                    return Some(Segment::Paused {
                        pcm: open.pcm.clone(),
                        pause: open.pauses,
                    });
                }
                None
            }
        }
    }

    /// When the open turn ends for lack of audio, in the call's milliseconds.
    pub(crate) fn deadline(&self) -> Option<u64> {
        self.open
            .as_ref()
            .map(|open| open.last_audio_ms + AUDIO_IDLE_MS)
    }

    /// Ends the open turn if no audio arrived for it since its deadline.
    pub(crate) fn poll(&mut self, now: u64) -> Option<Segment> {
        let open = self.open.as_ref()?;
        let idle = now.saturating_sub(open.last_audio_ms);
        (idle >= AUDIO_IDLE_MS).then(|| self.end(idle))
    }

    /// Ends the open turn if it is still in pause number `pause`: an end-of-turn model said it is over.
    pub(crate) fn end_paused(&mut self, pause: u32) -> Option<Segment> {
        let open = self.open.as_ref()?;
        let still = open.pauses == pause && open.silent > 0;
        let silence_ms = open.silent / PER_MS;
        still.then(|| self.end(silence_ms))
    }

    /// Ends the open turn now, as if its speech had stopped `silence_ms` ago.
    pub(crate) fn close(&mut self, silence_ms: u64) -> Option<Segment> {
        self.open.as_ref()?;
        Some(self.end(silence_ms))
    }

    /// Drops the open turn and the pre-roll.
    pub(crate) fn cancel(&mut self) {
        self.open = None;
        self.recent.clear();
    }

    fn end(&mut self, silence_ms: u64) -> Segment {
        let open = self.open.take().expect("an open turn");
        self.recent.clear();
        Segment::Ended(EndedTurn {
            pcm: open.pcm,
            silence_ms,
        })
    }
}

/// Milliseconds of 16 kHz audio in `samples`.
pub(crate) fn duration_ms(samples: usize) -> u64 {
    samples as u64 / PER_MS
}
