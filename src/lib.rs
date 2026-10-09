//! The voice call on the device: the microphone heard as turns of speech, transcribed through the Sidevoice engine
//! and reported to the room as text; the room's replies spoken through the engine, played, interrupted when the
//! person speaks over them, and reported as heard or unheard. It holds no socket: what it says to the room and what
//! the room says to it are messages its host carries.
//!
//! Inside: `config` ([`VoiceConfig`]), `room` (the room's messages), `event` (what a call tells its host), `turns`
//! (segmentation: detector windows into turns), `recognition` (the queue, the acceptance filter and the merge
//! window), `speech` (a reply in sentence chunks), `playback` (the queue, barge-in and the heard position) and `call`
//! (the state machine that runs the three regions).

mod call;
mod config;
mod event;
mod io;
mod maybe_send;
mod models;
mod playback;
mod recognition;
mod room;
mod runtime;
mod speech;
mod turns;
mod voice_call;
#[cfg(web)]
mod web;

#[cfg(test)]
mod test_support;

pub use config::{EndOfTurn, ListeningBar, Patience, Stage, SttStage, TtsStage, VoiceConfig};
pub use event::{CallState, Karaoke, Listening, PlaybackState, VoiceError, VoiceEvent};
pub use io::{AudioIo, IoEvent, IoSink};
pub use maybe_send::{MaybeSend, MaybeSync};
pub use room::{
    Playback, PlaybackStatus, Reply, RoomEvent, RoomMessage, TurnPhase, TurnTimings, UserTurn,
};
pub use voice_call::{Events, VoiceCall};
