//! What a call tells its host ([`VoiceEvent`]): the room messages to carry, where the call is, how loud the
//! microphone is, where the reader of a reply is, and what failed.

use serde::Serialize;

use crate::room::RoomMessage;

/// Something the host must know or do.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "kebab-case")]
pub enum VoiceEvent {
    /// A message to send to the room, through the host's outbox.
    RoomMessage(RoomMessage),
    /// Where the call is, when it changed.
    State(CallState),
    /// The microphone's smoothed level, from 0 to 1, once per detector window.
    Level(f32),
    /// Where the reader of a reply is.
    Karaoke(Karaoke),
    /// Something failed that a person may be told, as a stable code.
    Error(VoiceError),
}

/// Where the call is: its three regions, and whether the room is in reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CallState {
    /// The microphone.
    pub listening: Listening,
    /// How many of the person's turns wait for, or are in, transcription.
    pub recognising: usize,
    /// The speaker.
    pub playback: PlaybackState,
    /// Whether the room is in reach; turns reported while it is not say `offline`.
    pub online: bool,
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
    /// A reply is being spoken and nothing sounds yet.
    Synthesizing,
    /// A reply sounds.
    Playing,
}

/// Where the reader of a reply is. Positions are in characters (Unicode scalar values) of the reply's text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Karaoke {
    /// The reply.
    pub utterance_id: String,
    /// The chunk sounding now, from its first character to the one after its last; `None` between chunks.
    pub sounding: Option<(usize, usize)>,
    /// What was heard, from the start: the chunks played to their end.
    pub heard_chars: usize,
}

/// A failure a person may be told of, as a stable code the app translates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VoiceError {
    /// The code: `transcription-queue-full`, or the engine's own (`transcription-failed`, `synthesis-failed`, ...).
    pub code: String,
}
