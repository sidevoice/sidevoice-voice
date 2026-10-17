//! What a call tells its host ([`VoiceEvent`]): the person's turns, where the call is, how loud the microphone is, and
//! what failed, and the take of a start that asked for one. What becomes of something the call was asked to say comes
//! through its own handle (`say`).

use serde::Serialize;

use crate::take::Take;

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
    /// The rendered speaker's smoothed level, from 0 to 1, once per detector window, with a terminal zero.
    PlaybackLevel(f32),
    /// Something failed that a person may be told, as a stable code.
    Error(VoiceError),
    /// What the microphone gave since the call listened, with where the turns fell in it: once each stretch of
    /// listening of a start that asked for a take ends (a stop, a failure, a restart on other models), before the error
    /// that ended it and before the state that says the call stopped.
    Take(Take),
}

/// One step of a turn of the person's speech, under the call's own id for the turn.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "lowercase")]
pub enum TurnEvent {
    /// A qualified speech window began a tentative turn. It is not a user turn or a transcript.
    Candidate {
        candidate_id: String,
        /// Approximate capture time of the first sample, in Unix milliseconds.
        onset_at: u64,
        /// Speaker state copied when those capture samples entered the call, before async VAD.
        playback: PlaybackCapture,
    },
    /// The tentative run failed the existing turn qualification.
    #[serde(rename = "candidate-cancelled")]
    CandidateCancelled { candidate_id: String },
    /// The person started speaking.
    Started {
        turn_id: String,
        /// The tentative run that opened this turn, when one was observed.
        #[serde(skip_serializing_if = "Option::is_none")]
        candidate_id: Option<String>,
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

/// One say handle's confirmed speaker state at a capture boundary. Counts are Unicode scalars.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SayCapture {
    pub say_id: String,
    pub hearing: CaptureHearing,
    pub heard_chars: Option<usize>,
    /// Order in which a handle first began sound; absent when it never sounded.
    pub played_order: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureHearing {
    Complete,
    Partial,
    NotPlayed,
    Unknown,
}

/// Bounded playback history at the source audio cutoff. Missing old handles are unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlaybackCapture {
    /// False only when no preceding capture block can bound the sample onset.
    pub available: bool,
    /// Highest opaque host marker processed before these microphone samples.
    pub mark: u64,
    pub recent: Vec<SayCapture>,
    pub last_played: Option<SayCapture>,
    pub truncated: bool,
}

impl PlaybackCapture {
    pub(crate) fn unknown() -> Self {
        Self {
            available: false,
            mark: 0,
            recent: Vec::new(),
            last_played: None,
            truncated: false,
        }
    }
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
    /// Waiting for speech: no turn is open, or the person paused in the open one (their speech stopped, as the
    /// detector decides it), which goes on.
    Listening,
    /// The person speaks: a turn is open and its speech has not stopped.
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
