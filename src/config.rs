//! What a call is set up with ([`VoiceConfig`]): the language it listens for, the voice it speaks with, how patient
//! the end of a turn is, the grace before anything is said, the listening bar, and how long idle models stay loaded.
//! It names no model: which models fill the call's slots is the app's ([`VoiceModels`](crate::VoiceModels)). Read from JSON
//! strictly: an unknown key is an error.

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// How one call listens and speaks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceConfig {
    /// The language spoken, as a BCP 47 tag (`es`, `en-US`), given to the transcriber; `None` for it to detect it.
    #[serde(default)]
    pub language: Option<String>,
    /// The speaker's voice, by the speaker's own id; `None` for its default.
    #[serde(default)]
    pub voice: Option<String>,
    /// The speed of speech, 1.0 being the speaker's own.
    #[serde(default = "default_speed")]
    pub speed: f32,
    /// What ends a turn.
    #[serde(default)]
    pub end_of_turn: EndOfTurn,
    /// How long a pause ends a turn, and how long a finished turn waits for the next one to join it.
    #[serde(default)]
    pub patience: Patience,
    /// How long after a turn ends what is to be said waits before it starts, in milliseconds.
    #[serde(default = "default_audio_grace_ms")]
    pub audio_grace_ms: u32,
    /// How long the models stay loaded while the call is stopped, in minutes (0: they are dropped as it stops). The
    /// next start loads them again.
    #[serde(default = "default_idle_unload_minutes")]
    pub idle_unload_minutes: u32,
    /// How loud speech must be to count.
    #[serde(default)]
    pub listening_bar: ListeningBar,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            language: None,
            voice: None,
            speed: default_speed(),
            end_of_turn: EndOfTurn::default(),
            patience: Patience::default(),
            audio_grace_ms: default_audio_grace_ms(),
            idle_unload_minutes: default_idle_unload_minutes(),
            listening_bar: ListeningBar::default(),
        }
    }
}

/// What ends a turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EndOfTurn {
    /// A pause as long as the patience says, on the voice activity detector's output.
    #[default]
    Silence,
    /// The app's end-of-turn model ([`EndOfTurnModel`](crate::EndOfTurnModel)), asked at each pause; a pause as long
    /// as the patience's longest ends the turn anyway.
    SmartTurn,
}

/// How patient the call is with pauses: one word for a person to choose, which sets the numbers
/// ([`Patience::end_of_turn_silence_ms`], [`Patience::smart_turn_ms`], [`Patience::merge_window_ms`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Patience {
    /// Short pauses end a turn.
    Fast,
    /// The default.
    #[default]
    Normal,
    /// Long pauses are part of the turn.
    Calm,
}

impl Patience {
    /// How long silence must last, after the detector's own end of speech, to end a turn.
    #[must_use]
    pub fn end_of_turn_silence_ms(self) -> u32 {
        match self {
            Self::Fast => 2_000,
            Self::Normal => 2_500,
            Self::Calm => 3_500,
        }
    }

    /// With `smart-turn`: how long a pause lasts, after the detector's own end of speech, before the end-of-turn model is
    /// asked, and how long one ends the turn whatever the model says.
    #[must_use]
    pub fn smart_turn_ms(self) -> (u32, u32) {
        match self {
            Self::Fast => (600, 2_500),
            Self::Normal => (900, 3_000),
            Self::Calm => (1_300, 4_000),
        }
    }

    /// How long a transcript waits for the next turn to join it before it is reported; 0 reports it at once.
    #[must_use]
    pub fn merge_window_ms(self) -> u32 {
        match self {
            Self::Fast => 0,
            Self::Normal => 500,
            Self::Calm => 1_500,
        }
    }
}

/// How loud speech must be to count, as a level from 0 to 1: the window's RMS in dBFS, from −60 (0) to 0 (1),
/// smoothed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListeningBar {
    /// The bar while nothing plays.
    pub quiet: f32,
    /// The bar while something plays and no turn is open: what starts a barge-in.
    pub playing: f32,
}

impl Default for ListeningBar {
    fn default() -> Self {
        Self {
            quiet: 0.5,
            playing: 0.8,
        }
    }
}

fn default_audio_grace_ms() -> u32 {
    1_000
}

fn default_idle_unload_minutes() -> u32 {
    10
}

fn default_speed() -> f32 {
    1.0
}
