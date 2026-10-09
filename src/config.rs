//! What a call is set up with ([`VoiceConfig`]): the engine model of each stage, how patient the end of a turn is,
//! the grace before a reply, and the listening bar. Read from JSON strictly: an unknown or a missing key is an error.

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// How one call listens and speaks. The stages name engine models, local or remote alike: the engine has one
/// catalogue for both.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceConfig {
    /// The voice activity detector.
    pub vad: Stage,
    /// Speech to text.
    pub stt: SttStage,
    /// Text to speech.
    pub tts: TtsStage,
    /// What ends a turn.
    #[serde(default)]
    pub end_of_turn: EndOfTurn,
    /// How long a pause ends a turn, and how long a finished turn waits for the next one to join it.
    #[serde(default)]
    pub patience: Patience,
    /// How long after a turn ends a reply waits before it starts, in milliseconds.
    #[serde(default = "default_audio_grace_ms")]
    pub audio_grace_ms: u32,
    /// How loud speech must be to count.
    #[serde(default)]
    pub listening_bar: ListeningBar,
}

/// An engine model, and optionally the build of it to load (the engine's recommended one otherwise).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    /// The model's id in the engine's catalogue.
    pub model: String,
    /// The build's id, or `None` for the one the engine picks.
    #[serde(default)]
    pub build: Option<String>,
}

/// The speech-to-text stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SttStage {
    /// The model's id in the engine's catalogue.
    pub model: String,
    /// The build's id, or `None` for the one the engine picks.
    #[serde(default)]
    pub build: Option<String>,
    /// The language spoken, as a BCP 47 tag (`es`, `en-US`), or `None` for the model to detect it.
    #[serde(default)]
    pub language: Option<String>,
}

/// The text-to-speech stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TtsStage {
    /// The model's id in the engine's catalogue.
    pub model: String,
    /// The build's id, or `None` for the one the engine picks.
    #[serde(default)]
    pub build: Option<String>,
    /// The voice's id among the model's voices, or `None` for its default.
    #[serde(default)]
    pub voice: Option<String>,
    /// The speed, 1.0 being the model's own.
    #[serde(default = "default_speed")]
    pub speed: f32,
}

/// What ends a turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EndOfTurn {
    /// A pause as long as the patience says.
    #[default]
    Silence,
    /// The engine's `end-of-turn` model, asked at each pause (sidevoice-engine#69).
    SmartTurn,
}

/// How patient the call is with pauses: one word for a person to choose, which sets the numbers
/// ([`Patience::end_of_turn_silence_ms`], [`Patience::merge_window_ms`]).
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
    /// The bar while a reply plays and no turn is open: what starts a barge-in.
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

fn default_speed() -> f32 {
    1.0
}
