//! What a call tells its host ([`VoiceEvent`]): the person's turns, where the call is, how loud the microphone is, and
//! what failed. What becomes of something the call was asked to say comes through its own handle (`say`).

use serde::Serialize;

/// Something the host must know.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "kebab-case")]
pub enum VoiceEvent {
    /// A turn of the person's started, ended with its words, or came to nothing.
    Turn(TurnEvent),
    /// Where the call is, when it changed.
    State(CallState),
    /// The microphone's smoothed level, from 0 to 1, once per detector window.
    Level(f32),
    /// Something failed that a person may be told, as a stable code.
    Error(VoiceError),
}

/// One step of a turn of the person's speech, under the call's own id for the turn.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "lowercase")]
pub enum TurnEvent {
    /// The person started speaking.
    Started {
        turn_id: String,
        /// When, in Unix milliseconds.
        started_at: u64,
    },
    /// The turn's transcript is final: what the person said.
    Finished {
        turn_id: String,
        text: String,
        /// The language the transcript is in, when the transcriber was told one.
        #[serde(skip_serializing_if = "Option::is_none")]
        language: Option<String>,
        /// When the person started speaking (the first of the turns it joined) and when the turn ended, in Unix
        /// milliseconds.
        started_at: u64,
        ended_at: u64,
        /// Whether it joined earlier turns, which were cancelled with `merged`.
        merged: bool,
        timings: TurnTimings,
    },
    /// Nothing comes of the turn: no words in it, the person cancelled it, or it was joined into the next one
    /// (`merged`).
    Cancelled { turn_id: String, merged: bool },
}

/// How long a turn's parts took, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TurnTimings {
    /// The audio of the turn.
    pub audio_ms: u64,
    /// The silence that ended it.
    pub endpoint_silence_ms: u64,
    /// From the end of the turn to its transcript.
    pub recognition_ms: u64,
}

/// Where the call is: its three regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CallState {
    /// The microphone.
    pub listening: Listening,
    /// How many of the person's turns wait for, or are in, transcription.
    pub recognising: usize,
    /// The speaker.
    pub playback: PlaybackState,
    /// Whether the microphone gives audio.
    pub microphone: Microphone,
}

/// Whether the microphone gives audio, whatever the person's mute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Microphone {
    /// It gives audio, or the call is not started.
    Live,
    /// Its device gives none for now (a muted `MediaStreamTrack`: another app or the system holds it, or the device
    /// stopped delivering); the call hears silence until it is live again. Not the person's mute, which is
    /// [`Listening::Muted`].
    Muted,
}

/// Where the microphone is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Listening {
    /// The call is not started.
    Idle,
    /// Muted by the person.
    Muted,
    /// Waiting for speech.
    Listening,
    /// A turn is open.
    Speaking,
}

/// Where the speaker is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlaybackState {
    /// Nothing to say.
    Idle,
    /// Something is being spoken and nothing sounds yet.
    Synthesizing,
    /// Something sounds.
    Playing,
}

/// A failure a person may be told of, as a stable code the app translates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VoiceError {
    /// The code: `transcription-queue-full`, or the app's models' own (`transcription-failed`, `speech-failed`, ...).
    pub code: String,
}
