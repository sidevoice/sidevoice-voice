//! Segmentation: the detector's windows of 16 kHz audio become turns, as Pipecat ended them with Silero and smart-turn
//! (`pipecat-ai` 1.10.0: `VADAnalyzer`, `BaseSmartTurn`, `TurnAnalyzerUserTurnStopStrategy`,
//! `SpeechTimeoutUserTurnStopStrategy`).
//!
//! - A window is speech when the detector's confidence is at least [`CONFIDENCE`] and the volume (`volume`) clears the
//!   listening bar, raised while a reply plays. Speech starts after [`START_MS`] of such windows and stops after the
//!   segmentation's `stop_ms` without one (`activity`).
//! - A turn opens where speech starts, with the second of audio before it (the pre-roll).
//! - Where speech stops inside a turn, a pause begins. With `smart-turn` the turn's audio so far is offered to an
//!   end-of-turn model, at each stop; the turn ends when it says the turn is over, or once the pause has lasted
//!   `end_ms` whatever it says. With `silence`, the pause ends the turn once it has lasted `end_ms`. Speech that starts
//!   again ends the pause, and the turn goes on.
//! - A turn also ends when the audio stops arriving, and when it is closed or cancelled. It holds at most a minute of
//!   audio: what comes after is not kept.

mod activity;
mod level;
mod volume;

#[cfg(test)]
mod tests;

use std::collections::VecDeque;

use activity::{Activity, Change};
#[cfg(test)]
pub(crate) use level::instant;
use level::Level;
#[cfg(test)]
pub(crate) use volume::loudness;
use volume::Volume;

/// The rate segmentation works at, in Hz.
pub(crate) const RATE: u32 = 16_000;
/// Samples per millisecond.
const PER_MS: u64 = RATE as u64 / 1000;
/// The detector's confidence from which a window is speech.
const CONFIDENCE: f32 = 0.6;
/// How long speech must go on before it starts, in milliseconds.
const START_MS: u32 = 400;
/// With `silence`, how long the detector must hear no speech before it says speech stopped, in milliseconds.
pub(crate) const SILENCE_STOP_MS: u32 = 200;
/// The audio kept before a turn opens.
const PRE_ROLL: usize = RATE as usize;
/// The audio an end-of-turn model is given from before speech started: half a second, and the speech it took to
/// start.
const PRE_SPEECH: usize = (500 + START_MS as usize) * PER_MS as usize;
/// The most audio an end-of-turn model is given: the last eight seconds.
const END_OF_TURN_AUDIO: usize = RATE as usize * 8;
/// The most audio one turn keeps.
const MAX_TURN: usize = RATE as usize * 60;
/// How long an open turn waits for audio that stopped arriving (a muted or lost microphone) before it ends.
pub(crate) const AUDIO_IDLE_MS: u64 = 5_000;

/// The numbers segmentation runs on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Segmentation {
    /// How long the detector must hear no speech before it says speech stopped, in milliseconds.
    pub(crate) stop_ms: u32,
    /// How long a pause lasts, from that stop, before it ends the turn, in milliseconds.
    pub(crate) end_ms: u32,
    /// Whether each stop offers the turn's audio to an end-of-turn model (`smart-turn`).
    pub(crate) ask: bool,
    /// The volume speech needs.
    pub(crate) quiet_bar: f32,
    /// The volume speech needs while a reply plays, if higher.
    pub(crate) playing_bar: f32,
}

/// What one window did to the turn.
#[derive(Debug, PartialEq)]
pub(crate) enum Segment {
    /// A turn opened.
    Started,
    /// The open turn ended, with its audio.
    Ended(EndedTurn),
    /// Speech stopped in the open turn: its audio for an end-of-turn model, and the pause's number in the turn, to
    /// ask whether it is over.
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
    /// Samples heard since the window that opened it.
    heard: usize,
    /// Pauses so far, the one going on included.
    pauses: u32,
    /// Samples since speech stopped, while a pause goes on.
    paused: Option<u64>,
    /// Samples since the detector last held a window for speech.
    silent: u64,
    /// The time of the last window, in the call's milliseconds.
    last_audio_ms: u64,
}

/// Turns out of the detector's windows.
#[derive(Debug)]
pub(crate) struct Segmenter {
    numbers: Segmentation,
    level: Level,
    volume: Volume,
    activity: Activity,
    /// The speech that made speech start last, in samples: it began this long before the end of the window it started
    /// in.
    start_run: usize,
    /// The last audio heard, as much as an end-of-turn model is given: since the last turn ended, at most.
    recent: VecDeque<f32>,
    open: Option<Open>,
}

impl Segmenter {
    pub(crate) fn new(numbers: Segmentation) -> Self {
        Self {
            numbers,
            level: Level::default(),
            volume: Volume::default(),
            activity: Activity::default(),
            start_run: 0,
            recent: VecDeque::with_capacity(END_OF_TURN_AUDIO),
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

    /// Whether the open turn is in a pause: speech stopped in it and has not started again.
    pub(crate) fn paused(&self) -> bool {
        self.open.as_ref().is_some_and(|open| open.paused.is_some())
    }

    /// How long the detector has heard no speech in the open turn, in samples: where its speech stopped, before the
    /// end of the last window.
    pub(crate) fn silence(&self) -> usize {
        self.open.as_ref().map_or(0, |open| open.silent as usize)
    }

    /// The speech that made speech start last, in samples: where it began, before the end of the window it started in.
    pub(crate) fn start_run(&self) -> usize {
        self.start_run
    }

    /// The smoothed level of the last window, from 0 to 1.
    pub(crate) fn level(&self) -> f32 {
        self.level.value()
    }

    /// Takes one detector window at `now`: its samples and the detector's confidence that it is speech. `playing`
    /// says whether a reply is sounding, which raises the volume speech needs to the playing bar.
    pub(crate) fn window(
        &mut self,
        now: u64,
        pcm: &[f32],
        confidence: f32,
        playing: bool,
    ) -> Option<Segment> {
        self.level.take(pcm);
        let volume = self.volume.take(pcm);
        let bar = if playing {
            self.numbers.quiet_bar.max(self.numbers.playing_bar)
        } else {
            self.numbers.quiet_bar
        };
        let speech = confidence >= CONFIDENCE && volume >= bar;
        let change = self
            .activity
            .take(speech, pcm.len(), START_MS, self.numbers.stop_ms);
        if change == Some(Change::Started) {
            self.start_run = activity::windows(START_MS, pcm.len()).max(1) as usize * pcm.len();
        }
        self.recent.extend(pcm);
        let excess = self.recent.len().saturating_sub(END_OF_TURN_AUDIO);
        self.recent.drain(..excess);
        let Some(open) = &mut self.open else {
            if change != Some(Change::Started) {
                return None;
            }
            let from = self.recent.len().saturating_sub(PRE_ROLL + pcm.len());
            self.open = Some(Open {
                pcm: self.recent.range(from..).copied().collect(),
                heard: 0,
                pauses: 0,
                paused: None,
                silent: 0,
                last_audio_ms: now,
            });
            return Some(Segment::Started);
        };
        open.last_audio_ms = now;
        open.heard += pcm.len();
        let room = MAX_TURN.saturating_sub(open.pcm.len());
        open.pcm.extend_from_slice(&pcm[..pcm.len().min(room)]);
        open.silent = if self.activity.speaking() {
            0
        } else {
            open.silent + pcm.len() as u64
        };
        match change {
            Some(Change::Started) => open.paused = None,
            Some(Change::Stopped) => {
                open.pauses += 1;
                open.paused = Some(0);
                if self.numbers.ask {
                    let length = (open.heard + PRE_SPEECH).min(END_OF_TURN_AUDIO);
                    let from = self.recent.len().saturating_sub(length);
                    return Some(Segment::Paused {
                        pcm: self.recent.range(from..).copied().collect(),
                        pause: open.pauses,
                    });
                }
            }
            None => {
                if let Some(paused) = &mut open.paused {
                    *paused += pcm.len() as u64;
                    if *paused >= u64::from(self.numbers.end_ms) * PER_MS {
                        let silence_ms = open.silent / PER_MS;
                        return Some(self.end(silence_ms));
                    }
                }
            }
        }
        None
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
        if idle < AUDIO_IDLE_MS {
            return None;
        }
        self.activity.reset();
        Some(self.end(idle))
    }

    /// Ends the open turn if it is still in pause number `pause`: an end-of-turn model said it is over.
    pub(crate) fn end_paused(&mut self, pause: u32) -> Option<Segment> {
        let open = self.open.as_ref()?;
        let still = open.pauses == pause && open.paused.is_some();
        let silence_ms = open.silent / PER_MS;
        still.then(|| self.end(silence_ms))
    }

    /// Ends the open turn now, as if its speech had stopped `silence_ms` ago; the speech going on is not heard again
    /// until it starts anew.
    pub(crate) fn close(&mut self, silence_ms: u64) -> Option<Segment> {
        self.open.as_ref()?;
        self.activity.reset();
        Some(self.end(silence_ms))
    }

    /// Drops the open turn and the audio before it; the speech going on is not heard again until it starts anew.
    pub(crate) fn cancel(&mut self) {
        self.open = None;
        self.recent.clear();
        self.activity.reset();
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
