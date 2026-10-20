//! What a call is set up with, as data read strictly from JSON (an unknown key is an error): its halves, each optional
//! ([`InputConfig`]: the language it listens for and what ends a turn; [`OutputConfig`]: the voice it speaks with), and
//! what holds for the whole call ([`VoiceConfig`]: the grace before anything is said, how long idle models stay
//! loaded). It names no model: which models fill each half's slots is the app's
//! ([`InputModels`](crate::InputModels), [`OutputModels`](crate::OutputModels)).

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// What holds for the whole call, whichever halves it has.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceConfig {
    /// How long after a turn ends what is to be said waits before it starts, in milliseconds.
    #[serde(default = "default_audio_grace_ms", deserialize_with = "grace_ms")]
    pub audio_grace_ms: u32,
    /// How long the models stay loaded while the call is stopped, in minutes (0: they are dropped as it stops). The
    /// next start loads them again.
    #[serde(default = "default_idle_unload_minutes")]
    pub idle_unload_minutes: u32,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            audio_grace_ms: default_audio_grace_ms(),
            idle_unload_minutes: default_idle_unload_minutes(),
        }
    }
}

/// How the input half listens: the microphone, the detector, the transcriber and, with [`EndOfTurn::Smart`], the
/// end-of-turn model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputConfig {
    /// The language spoken, as a BCP 47 tag (`es`, `en-US`), given to the transcriber; `None` for it to detect it.
    #[serde(default)]
    pub language: Option<String>,
    /// What ends a turn.
    #[serde(default)]
    pub end_of_turn: EndOfTurn,
    /// How long a pause ends a turn, and how long a finished turn waits for the next one to join it.
    #[serde(default)]
    pub patience: Patience,
    /// How loud speech must be to count.
    #[serde(default)]
    pub listening_bar: ListeningBar,
    /// How much qualified speech interrupts sounding audio, in seconds, counted wherever it falls in the last two and a
    /// half times as long: 0.6 s by default, so bursts with pauses between them add up.
    #[serde(default)]
    pub barge_in_min_speech: BargeInMinSpeech,
    /// The sliding window the minimum interrupting speech falls in, as a multiple of that minimum, from 1 to 10: 2.5 by
    /// default.
    #[serde(
        default = "default_barge_in_window",
        deserialize_with = "barge_in_window"
    )]
    pub barge_in_window: f32,
    /// The detector's confidence from which a window is speech, above 0 and at most 1: 0.6 by default.
    #[serde(
        default = "default_vad_confidence",
        deserialize_with = "vad_confidence"
    )]
    pub vad_confidence: f32,
    /// How long speech must go on before it starts a turn in silence, in milliseconds, at most 5000: 400 by default.
    #[serde(
        default = "default_speech_start_ms",
        deserialize_with = "speech_start_ms"
    )]
    pub speech_start_ms: u32,
    /// The audio kept before a turn opens, in milliseconds, at most 5000: 1000 by default.
    #[serde(default = "default_pre_roll_ms", deserialize_with = "pre_roll_ms")]
    pub pre_roll_ms: u32,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            language: None,
            end_of_turn: EndOfTurn::default(),
            patience: Patience::default(),
            listening_bar: ListeningBar::default(),
            barge_in_min_speech: BargeInMinSpeech::default(),
            barge_in_window: default_barge_in_window(),
            vad_confidence: default_vad_confidence(),
            speech_start_ms: default_speech_start_ms(),
            pre_roll_ms: default_pre_roll_ms(),
        }
    }
}

/// How the output half speaks: the speaker model and the playback.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputConfig {
    /// The language of what is said when `say` gives none, as a BCP 47 tag; `None`: the input's language, if any.
    #[serde(default)]
    pub language: Option<String>,
    /// The speaker's voice, by the speaker's own id; `None` for its default.
    #[serde(default)]
    pub voice: Option<String>,
    /// The speed of speech, 1.0 being the speaker's own.
    #[serde(default = "default_speed")]
    pub speed: f32,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            language: None,
            voice: None,
            speed: default_speed(),
        }
    }
}

/// The whole configuration of one call, as its state machine reads it: which halves it has, and how each works.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct CallConfig {
    pub(crate) input: Option<InputConfig>,
    pub(crate) output: Option<OutputConfig>,
    pub(crate) call: VoiceConfig,
}

impl CallConfig {
    /// The language the transcriber is given.
    pub(crate) fn input_language(&self) -> Option<String> {
        self.input.as_ref().and_then(|input| input.language.clone())
    }

    /// The language what is said is in when `say` gives none: the output's, else the input's.
    pub(crate) fn say_language(&self) -> Option<String> {
        self.output
            .as_ref()
            .and_then(|output| output.language.clone())
            .or_else(|| self.input_language())
    }

    /// The input half's numbers, or the defaults when there is none (nothing is heard then).
    pub(crate) fn input_or_default(&self) -> InputConfig {
        self.input.clone().unwrap_or_default()
    }
}

/// What ends a turn: `{"kind": "silence", "ms"?}` or `{"kind": "smart"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum EndOfTurn {
    /// A pause on the voice activity detector's output: `ms` after the detector says speech stopped (200 ms without
    /// it); the patience's [`Patience::silence_ms`] when absent. Needs no end-of-turn model, and none is loaded.
    Silence {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ms: Option<u32>,
    },
    /// The app's end-of-turn model ([`EndOfTurnModel`](crate::EndOfTurnModel)), asked at each pause; a pause as long
    /// as the patience's longest ends the turn anyway.
    Smart {},
}

impl Default for EndOfTurn {
    fn default() -> Self {
        Self::Silence { ms: None }
    }
}

/// How patient the call is with pauses: one word for a person to choose, which sets the numbers
/// ([`Patience::smart_turn_stop_ms`], [`Patience::smart_turn_longest_ms`], [`Patience::silence_ms`],
/// [`Patience::merge_window_ms`]).
///
/// `normal` is the numbers Pipecat ended turns with in the Python core (its Silero detector's `stop_secs` of 0.9 s,
/// smart-turn's `stop_secs` of 3 s, the speech timeout of 2.5 s). `fast` and `calm` are that core's own variants of
/// them, about a third shorter and longer: 0.6, 2.5 and 2 s; 1.3, 4 and 3.5 s.
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
    /// With `smart`: how long the detector must hear no speech before it says speech stopped, which is when the
    /// end-of-turn model is asked.
    #[must_use]
    pub fn smart_turn_stop_ms(self) -> u32 {
        match self {
            Self::Fast => 600,
            Self::Normal => 900,
            Self::Calm => 1_300,
        }
    }

    /// With `smart`: how long a pause lasts, after the detector said speech stopped, before it ends the turn
    /// whatever the end-of-turn model said.
    #[must_use]
    pub fn smart_turn_longest_ms(self) -> u32 {
        match self {
            Self::Fast => 2_500,
            Self::Normal => 3_000,
            Self::Calm => 4_000,
        }
    }

    /// With `silence`: how long a pause lasts, after the detector said speech stopped (200 ms without it), before it
    /// ends the turn.
    #[must_use]
    pub fn silence_ms(self) -> u32 {
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

/// How loud speech must be to count, as a volume from 0 to 1 the way Pipecat's detector measures it: the loudness of
/// the last 400 ms (ITU-R BS.1770), from −110 LUFS (0) to −10 LUFS (1), smoothed. A window the detector holds for
/// speech under the bar is silence.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, try_from = "RawListeningBar")]
pub struct ListeningBar {
    /// The bar while nothing plays: −60 LUFS by default.
    pub quiet: f32,
    /// The bar while something plays, when it is higher: what a barge-in must clear, −45 LUFS by default.
    pub playing: f32,
}

impl Default for ListeningBar {
    fn default() -> Self {
        Self {
            quiet: default_quiet_bar(),
            playing: default_playing_bar(),
        }
    }
}

/// A listening bar as read, before its volumes are checked.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawListeningBar {
    #[serde(default = "default_quiet_bar")]
    quiet: f32,
    #[serde(default = "default_playing_bar")]
    playing: f32,
}

impl TryFrom<RawListeningBar> for ListeningBar {
    type Error = String;

    fn try_from(raw: RawListeningBar) -> Result<Self, String> {
        for (name, value) in [("quiet", raw.quiet), ("playing", raw.playing)] {
            if !(value.is_finite() && (0.0..=1.0).contains(&value)) {
                return Err(format!("listening_bar.{name} must be between 0 and 1"));
            }
        }
        Ok(Self {
            quiet: raw.quiet,
            playing: raw.playing,
        })
    }
}

/// Reads a number that must lie in `low..=high` (and be finite), or fails naming `name`.
fn ranged<'de, D, T>(deserializer: D, name: &str, low: T, high: T) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + PartialOrd + Copy + std::fmt::Display + Finite,
{
    let value = T::deserialize(deserializer)?;
    if value.finite() && value >= low && value <= high {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(format!(
            "{name} must be between {low} and {high}"
        )))
    }
}

/// Whether a number is a number: floats are not NaN or infinite, integers always are.
trait Finite {
    fn finite(&self) -> bool;
}

impl Finite for f32 {
    fn finite(&self) -> bool {
        self.is_finite()
    }
}

impl Finite for u32 {
    fn finite(&self) -> bool {
        true
    }
}

fn grace_ms<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    ranged(deserializer, "audio_grace_ms", 0, 60_000)
}

fn barge_in_window<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    ranged(deserializer, "barge_in_window", 1.0, 10.0)
}

/// Above zero: a confidence of 0 would make every window speech.
fn vad_confidence<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    let value = ranged(deserializer, "vad_confidence", 0.0, 1.0)?;
    if value > 0.0 {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "vad_confidence must be above 0 and at most 1",
        ))
    }
}

fn speech_start_ms<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    ranged(deserializer, "speech_start_ms", 0, 5_000)
}

fn pre_roll_ms<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    ranged(deserializer, "pre_roll_ms", 0, 5_000)
}

fn default_quiet_bar() -> f32 {
    0.5
}

fn default_playing_bar() -> f32 {
    0.65
}

fn default_barge_in_window() -> f32 {
    2.5
}

fn default_vad_confidence() -> f32 {
    0.6
}

fn default_speech_start_ms() -> u32 {
    400
}

fn default_pre_roll_ms() -> u32 {
    1_000
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

/// A validated interruption duration, serialized as seconds. Native callers construct it with [`Self::new`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(transparent)]
pub struct BargeInMinSpeech(f32);

impl BargeInMinSpeech {
    /// Accepts finite seconds above zero and at most one minute.
    pub fn new(seconds: f32) -> Result<Self, &'static str> {
        if seconds.is_finite() && seconds > 0.0 && seconds <= 60.0 {
            Ok(Self(seconds))
        } else {
            Err("barge_in_min_speech must be between 0 and 60 seconds")
        }
    }

    /// The configured duration in seconds.
    pub fn seconds(self) -> f32 {
        self.0
    }
}

impl Default for BargeInMinSpeech {
    fn default() -> Self {
        Self(0.6)
    }
}

impl<'de> Deserialize<'de> for BargeInMinSpeech {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let seconds = f32::deserialize(deserializer)?;
        Self::new(seconds).map_err(serde::de::Error::custom)
    }
}
