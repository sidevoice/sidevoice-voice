//! Segmentation: the detector's windows of 16 kHz audio become turns, as Pipecat ended them with Silero and smart-turn
//! (`pipecat-ai` 1.10.0: `VADAnalyzer`, `BaseSmartTurn`, `TurnAnalyzerUserTurnStopStrategy`,
//! `SpeechTimeoutUserTurnStopStrategy`).
//!
//! - A window is speech when the detector's confidence is at least the configured confidence and the volume (`volume`) clears the
//!   listening bar, raised while a reply sounds. In silence speech starts after the configured start time of such windows; over
//!   sounding audio it starts only once the configured minimum of such windows has been heard within a sliding window two and
//!   a half times as long (`overlap`), wherever they fall in it, so bursts with pauses add up. It stops after the
//!   segmentation's `stop_ms` without one (`activity`).
//! - Playback starting or stopping with no turn open does not restart the count: what was heard on one side is
//!   credited to the other (`carry`), so an interruption that straddles a gap between a reply's sentences opens no
//!   later than one that does not.
//! - A turn opens where speech starts, with the second of audio before it (the pre-roll). Over sounding audio the
//!   pre-roll is the audio the detector held for speech before the first window that cleared the bar, however soft:
//!   the first word of an interruption is kept, the playback before it is not.
//! - Where speech stops inside a turn, a pause begins. With `smart-turn` the turn's audio so far is offered to an
//!   end-of-turn model, at each stop; the turn ends when it says the turn is over, or once the pause has lasted
//!   `end_ms` whatever it says. With `silence`, the pause ends the turn once it has lasted `end_ms`. Speech that starts
//!   again ends the pause, and the turn goes on.
//! - A turn also ends when the audio stops arriving, and when it is closed or cancelled.
//! - A long turn goes to recognition in pieces, none of them longer than a recogniser takes at once, and nothing of it
//!   is left out: once its audio reaches 15 s, it is cut in the middle of the next gap of 100 ms or more in its speech,
//!   where speech goes on again; at 25 s with no such gap, it is cut where it is, unless it is in a pause. The turn goes
//!   on with what follows. What is left after a piece when the turn ends, if no speech was heard in it or it lasts no
//!   more than 300 ms, is no audio to recognise.

mod activity;
mod overlap;
mod volume;

#[cfg(test)]
mod tests;

use std::collections::VecDeque;

#[cfg(test)]
pub(crate) use crate::level::instant;
use crate::level::Level;
use activity::{Activity, Change};
use overlap::Overlap;
#[cfg(test)]
pub(crate) use volume::loudness;
use volume::Volume;

/// The rate segmentation works at, in Hz.
pub(crate) const RATE: u32 = 16_000;
/// Samples per millisecond.
const PER_MS: u64 = RATE as u64 / 1000;
/// With `silence`, how long the detector must hear no speech before it says speech stopped, in milliseconds.
pub(crate) const SILENCE_STOP_MS: u32 = 200;
/// The audio an end-of-turn model is given from before speech started, besides the speech it took to start: half a
/// second, in milliseconds.
const PRE_SPEECH_MS: usize = 500;
/// The most audio an end-of-turn model is given: the last eight seconds.
const END_OF_TURN_AUDIO: usize = RATE as usize * 8;
/// From how long a turn's audio is cut into a piece in the next gap in its speech, in samples. Recognisers take a
/// limited length at once: Whisper hears 30-second windows, and some builds of it keep only the first.
const PIECE_FROM: usize = RATE as usize * 15;
/// How long a turn's audio gets before it is cut into a piece without a gap, in samples.
const PIECE_MAX: usize = RATE as usize * 25;
/// The shortest gap in speech a piece is cut in, in samples: 100 ms.
const PIECE_GAP: usize = RATE as usize / 10;
/// The shortest audio a turn's last piece is recognised with, in samples: 300 ms.
const PIECE_MIN: usize = RATE as usize * 3 / 10;
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
    /// Qualified speech required while output is sounding, in milliseconds.
    pub(crate) barge_in_min_ms: u32,
    /// The sliding window that speech falls in, as a multiple of the minimum.
    pub(crate) barge_in_window: f32,
    /// The detector's confidence from which a window is speech.
    pub(crate) confidence: f32,
    /// How long speech must go on before it starts, in milliseconds.
    pub(crate) start_ms: u32,
    /// The audio kept before a turn opens, in milliseconds.
    pub(crate) pre_roll_ms: u32,
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
    /// The open turn's audio since its last piece, cut off to be recognised on its own: the turn goes on, and what it
    /// holds next follows this audio without a sample left out.
    Piece(Vec<f32>),
}

/// The audio of a turn that ended.
#[derive(Debug, PartialEq)]
pub(crate) struct EndedTurn {
    /// 16 kHz mono samples since its last piece, the pre-roll included when it was cut into none; none when, after a
    /// piece, nothing voiced or no more than 300 ms of audio was left.
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
    /// Whether a piece was cut off it.
    cut: bool,
    /// Whether speech was heard in the audio held since the last piece (the turn's opening speech, if none was cut).
    voiced: bool,
}

impl Open {
    /// Cuts off the audio held so far as a piece, if it is due one, after a window of `window` samples: `quiet` samples
    /// without speech came before the window, and speech went on again `resumed` samples before the window's end, if it
    /// did. From [`PIECE_FROM`] the cut falls in the middle of a gap of at least [`PIECE_GAP`]; at [`PIECE_MAX`] it takes
    /// everything held, though never in a pause (where the turn may end, and leave a piece of nothing but silence).
    /// What follows the cut stays held.
    fn piece(&mut self, window: usize, quiet: u64, resumed: Option<usize>) -> Option<Vec<f32>> {
        let held = self.pcm.len();
        if let Some(run) = resumed {
            let gap_from = held.saturating_sub(window + quiet as usize);
            let gap = held.saturating_sub(run).saturating_sub(gap_from);
            let cut = gap_from + gap / 2;
            if gap >= PIECE_GAP && cut >= PIECE_FROM {
                self.cut = true;
                return Some(self.pcm.drain(..cut).collect());
            }
        }
        if held < PIECE_MAX || self.paused.is_some() {
            return None;
        }
        self.cut = true;
        self.voiced = false;
        Some(std::mem::take(&mut self.pcm))
    }
}

/// Turns out of the detector's windows.
#[derive(Debug)]
pub(crate) struct Segmenter {
    numbers: Segmentation,
    level: Level,
    volume: Volume,
    activity: Activity,
    /// The speech that made speech start last, in samples: it began this long before the end of the window it started
    /// in (over playback, the audio the opened turn kept).
    start_run: usize,
    /// Whether the preceding window was gated by audible playback.
    was_playing: bool,
    /// The speech heard over playback while no turn is open, in a sliding window.
    overlap: Overlap,
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
            was_playing: false,
            overlap: Overlap::default(),
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
        if self.was_playing != playing && self.open.is_none() {
            self.carry(playing, pcm.len());
        }
        self.was_playing = playing;
        self.level.take(pcm);
        let volume = self.volume.take(pcm);
        let bar = if playing {
            self.numbers.quiet_bar.max(self.numbers.playing_bar)
        } else {
            self.numbers.quiet_bar
        };
        let held = confidence >= self.numbers.confidence;
        let speech = held && volume >= bar;
        let gated = playing && self.open.is_none();
        let min_speech = self.numbers.barge_in_min_ms as usize * PER_MS as usize;
        let mut starting = 0;
        let change = if gated {
            self.overlap.take(
                pcm.len(),
                held,
                speech,
                overlap::window(min_speech, self.numbers.barge_in_window),
            );
            if speech && self.overlap.speech() >= min_speech {
                // The duration is counted from capture samples. Only now does VAD open a turn.
                self.activity.take(true, pcm.len(), 0, self.numbers.stop_ms)
            } else {
                self.activity.reset();
                None
            }
        } else {
            self.overlap.clear();
            starting = self.activity.starting();
            self.activity.take(
                speech,
                pcm.len(),
                self.numbers.start_ms,
                self.numbers.stop_ms,
            )
        };
        if change == Some(Change::Started) {
            self.start_run = if gated {
                self.overlap
                    .kept(self.numbers.pre_roll_ms as usize * PER_MS as usize)
            } else {
                (starting + 1) * pcm.len()
            };
        }
        // Only the audio from the first window the detector held for speech is eligible for pre-roll while playback
        // sounds, soft or not: an opening word under the playing bar belongs to the interruption, the playback before
        // it does not. Overlap that never opens a turn has no path to recognition or take marks; the microphone level
        // still reports normally.
        if gated {
            self.recent.extend(pcm);
            let excess = self.recent.len().saturating_sub(self.overlap.total());
            self.recent.drain(..excess);
        } else {
            self.recent.extend(pcm);
            let excess = self.recent.len().saturating_sub(END_OF_TURN_AUDIO);
            self.recent.drain(..excess);
        }
        let Some(open) = &mut self.open else {
            if change != Some(Change::Started) {
                return None;
            }
            let pre_roll = self.numbers.pre_roll_ms as usize * PER_MS as usize;
            // Over playback the turn keeps what the overlap kept; `recent` holds no more than that and then some.
            let keep = if gated {
                self.start_run
            } else {
                pre_roll.max(self.start_run) + pcm.len()
            };
            let from = self.recent.len().saturating_sub(keep);
            self.open = Some(Open {
                pcm: self.recent.range(from..).copied().collect(),
                heard: 0,
                pauses: 0,
                paused: None,
                silent: 0,
                last_audio_ms: now,
                cut: false,
                voiced: true,
            });
            return Some(Segment::Started);
        };
        open.last_audio_ms = now;
        open.heard += pcm.len();
        open.pcm.extend_from_slice(pcm);
        open.voiced |= self.activity.speaking();
        let quiet = open.silent;
        open.silent = if self.activity.speaking() {
            0
        } else {
            open.silent + pcm.len() as u64
        };
        // Where speech goes on again in this window, from its end: the run that started it, or this window after a gap.
        let resumed = match change {
            Some(Change::Started) => Some(self.start_run),
            _ if self.activity.speaking() && quiet > 0 => Some(pcm.len()),
            _ => None,
        };
        match change {
            Some(Change::Started) => open.paused = None,
            Some(Change::Stopped) => {
                open.pauses += 1;
                open.paused = Some(0);
                if !self.numbers.ask {
                    return None;
                }
                let pre_speech = (PRE_SPEECH_MS + self.numbers.start_ms as usize) * PER_MS as usize;
                let length = (open.heard + pre_speech).min(END_OF_TURN_AUDIO);
                let from = self.recent.len().saturating_sub(length);
                return Some(Segment::Paused {
                    pcm: self.recent.range(from..).copied().collect(),
                    pause: open.pauses,
                });
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
        open.piece(pcm.len(), quiet, resumed).map(Segment::Piece)
    }

    /// Playback started or stopped with no turn open, in a window of `window` samples: the speech heard so far keeps
    /// counting on the other side, so an interruption that straddles the switch is no slower than one that does not.
    /// From silence to playback, the windows of speech counted towards starting are credited to the overlap, as speech
    /// that qualified; from playback to silence, the speech in the overlap is credited towards starting, if it goes on
    /// up to the switch. The audio they were heard in stays in `recent`, so the turn keeps it.
    fn carry(&mut self, playing: bool, window: usize) {
        let window = window.max(1);
        if playing {
            let heard = self.activity.starting().min(self.recent.len() / window);
            let min_speech = self.numbers.barge_in_min_ms as usize * PER_MS as usize;
            let limit = overlap::window(min_speech, self.numbers.barge_in_window);
            self.overlap.clear();
            for _ in 0..heard {
                self.overlap.take(window, true, true, limit);
            }
            self.activity.reset();
        } else {
            let heard = if self.overlap.speaking() {
                self.overlap.speech() / window
            } else {
                0
            };
            self.overlap.clear();
            self.activity.credit(heard);
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

    /// Ends the open turn now with the audio it holds, if one is open: the person is not heard to stop, so the speech
    /// going on is not heard again until it starts anew (as when a turn ends for lack of audio).
    pub(crate) fn end_now(&mut self) -> Option<Segment> {
        let silence_ms = self.open.as_ref()?.silent / PER_MS;
        self.activity.reset();
        Some(self.end(silence_ms))
    }

    /// Drops the open turn and the audio before it; the speech going on is not heard again until it starts anew.
    pub(crate) fn cancel(&mut self) {
        self.open = None;
        self.recent.clear();
        self.activity.reset();
        self.was_playing = false;
        self.overlap.clear();
    }

    /// Forget a tentative speech run when the input or output changes without a detector window.
    /// An already-open turn keeps its audio and can still finish normally.
    pub(crate) fn reset_candidate(&mut self) {
        if self.open.is_none() {
            self.activity.reset();
            self.recent.clear();
            self.overlap.clear();
            self.was_playing = false;
            self.start_run = 0;
        }
    }

    fn end(&mut self, silence_ms: u64) -> Segment {
        let open = self.open.take().expect("an open turn");
        self.recent.clear();
        // After a piece, what is left with no speech in it, or too short to hold a word, is no audio to recognise.
        let nothing = open.cut && (!open.voiced || open.pcm.len() < PIECE_MIN);
        Segment::Ended(EndedTurn {
            pcm: if nothing { Vec::new() } else { open.pcm },
            silence_ms,
        })
    }
}

/// Milliseconds of 16 kHz audio in `samples`.
pub(crate) fn duration_ms(samples: usize) -> u64 {
    samples as u64 / PER_MS
}
