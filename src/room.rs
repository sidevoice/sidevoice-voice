//! The room's messages, as the module emits and consumes them: serde types and nothing else. The module never
//! holds a socket; its host carries these, keeps them in its outbox until the room acknowledges each by its
//! `client_msg_id`, and hands the room's own messages back.
//!
//! On the wire every message is `{"type": ..., "data": {...}}`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(test)]
mod tests;

/// What the module tells the room.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum RoomMessage {
    /// A turn of the person's speech: started, then finished (what becomes the conversation's row) or cancelled.
    #[serde(rename = "voice-user-turn")]
    UserTurn(UserTurn),
    /// What became of a reply: playing, then heard, interrupted, unplayed or failed.
    #[serde(rename = "voice-playback")]
    Playback(Playback),
}

impl RoomMessage {
    /// The message's id, for the host's outbox and the room's acknowledgement.
    #[must_use]
    pub fn client_msg_id(&self) -> &str {
        match self {
            Self::UserTurn(turn) => &turn.client_msg_id,
            Self::Playback(playback) => &playback.client_msg_id,
        }
    }

    /// The message as the room reads it.
    #[must_use]
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).expect("room messages serialize")
    }
}

/// One phase of a turn of the person's speech.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UserTurn {
    /// This message's id.
    pub client_msg_id: String,
    /// The turn's id, made by the module as the turn starts: the same in every phase of one turn, and how the room
    /// knows the turn (it answers `started` with it).
    pub turn_id: String,
    /// Where the turn is.
    pub phase: TurnPhase,
    /// What was said: in `finished`, and in a `cancelled` that was merged into the next turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The language the transcript is in, when the stage was told one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Whether the room was out of reach when the turn started. Such a turn is reported only as `finished`, which the
    /// room takes as words said while away.
    pub offline: bool,
    /// When the person started speaking, in Unix milliseconds.
    pub started_at: u64,
    /// When the turn ended, in Unix milliseconds; absent while it is open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    /// Whether this turn joined others (`finished`), or was joined into the next (`cancelled`).
    pub merged: bool,
    /// How long each part took, in milliseconds; present once the turn has ended.
    #[serde(rename = "timings_ms", skip_serializing_if = "Option::is_none")]
    pub timings: Option<TurnTimings>,
}

/// The phase of a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnPhase {
    /// The person started speaking.
    Started,
    /// Nothing comes of it: no speech in it, it was cancelled, or it was joined into the next turn.
    Cancelled,
    /// Its transcript is final.
    Finished,
}

/// How long a turn's parts took, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TurnTimings {
    /// The audio of the turn.
    pub audio_ms: u64,
    /// The silence that ended it.
    pub endpoint_silence_ms: u64,
    /// From the end of the turn to its transcript.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recognition_ms: Option<u64>,
}

/// What became of a reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Playback {
    /// This message's id.
    pub client_msg_id: String,
    /// The reply's id, as the room sent it.
    pub utterance_id: String,
    /// Where it is.
    pub status: PlaybackStatus,
    /// How much of its text was heard, in characters (Unicode scalar values) from its start.
    pub heard_chars: usize,
    /// Why it stopped short or never played. A failure has none: its stable code goes to the host as an error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<PlaybackReason>,
    /// When, in Unix milliseconds.
    pub at: u64,
}

/// What became of a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackStatus {
    /// Its first sound reached the speaker.
    Playing,
    /// It played to its end.
    Heard,
    /// The person spoke over it, or the call stopped, while it played.
    Interrupted,
    /// It never played: dropped before its turn came.
    Unplayed,
    /// It could not be spoken.
    Failed,
}

/// Why a reply stopped short or never played, in the room's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackReason {
    /// The person spoke over it.
    UserInterrupted,
    /// The person started a turn after it was written, or before its turn came.
    NewerTurn,
    /// The call stopped.
    CallEnded,
}

/// What the room sends that the module takes.
#[derive(Debug, Clone, PartialEq)]
pub enum RoomEvent {
    /// A reply to speak.
    Reply(Reply),
    /// The room took a turn of the person's (`turn_id`, the module's own), at `revision`: the turn's boundary. A reply
    /// written below it answers an older turn. `session_id` is the room session that `revision` counts in (opaque,
    /// compared only for equality): the same after a resume, another for a session that replaced it, whose revisions
    /// start again from 0.
    TurnStarted {
        session_id: String,
        turn_id: String,
        revision: u64,
    },
    /// The room refused a turn's `started` (message `client_msg_id`) because the call already has as many turns open as
    /// it keeps (`room.turns_full`): the turn is to be said again once one of them ends.
    TurnsFull { client_msg_id: String },
    /// Anything else the room sends, which the module does not use.
    Other,
}

impl RoomEvent {
    /// Reads one room message.
    ///
    /// # Errors
    ///
    /// When it is not `{"type", "data"}`, or a message the module takes lacks one of its fields.
    pub fn from_json(message: &Value) -> Result<Self, serde_json::Error> {
        #[derive(Deserialize)]
        struct Envelope {
            #[serde(rename = "type")]
            kind: String,
            #[serde(default)]
            data: Value,
        }
        let envelope = Envelope::deserialize(message)?;
        match envelope.kind.as_str() {
            "voice-reply" => Reply::deserialize(envelope.data).map(Self::Reply),
            "voice-user-turn" if envelope.data["phase"] == "started" => {
                #[derive(Deserialize)]
                struct Started {
                    session_id: String,
                    turn_id: String,
                    revision: u64,
                }
                Started::deserialize(envelope.data).map(|started| Self::TurnStarted {
                    session_id: started.session_id,
                    turn_id: started.turn_id,
                    revision: started.revision,
                })
            }
            "error" if envelope.data["key"] == "room.turns_full" => {
                #[derive(Deserialize)]
                struct Refused {
                    client_msg_id: String,
                }
                Refused::deserialize(envelope.data).map(|refused| Self::TurnsFull {
                    client_msg_id: refused.client_msg_id,
                })
            }
            _ => Ok(Self::Other),
        }
    }
}

/// A reply the room wants spoken.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Reply {
    /// Its id, which the module's `voice-playback` reports carry.
    pub utterance_id: String,
    /// The room's revision the reply was written at.
    pub revision: u64,
    /// The revision of the reply itself.
    pub reply_revision: u64,
    /// The conversation's thread.
    pub thread_id: String,
    /// The conversation row it speaks.
    pub history_id: String,
    /// What to say.
    pub text: String,
    /// The language it is written in.
    pub language: Option<String>,
    /// Whether it is said again because the person asked.
    #[serde(default)]
    pub replay: bool,
}
