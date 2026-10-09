//! The voice call on the device: the microphone heard as turns of speech, transcribed and reported to the room as
//! text; the room's replies spoken, played, interrupted when the person speaks over them, and reported as heard or
//! unheard. The models it runs are the app's, through the module's own interfaces (`models`: [`VoiceModels`], [`Vad`],
//! [`Transcriber`], [`Speaker`], [`EndOfTurnModel`]); nothing here names or links one. It holds no socket: what it says to the room and what
//! the room says to it are messages its host carries.
//!
//! Inside: `config` ([`VoiceConfig`]), `room` (the room's messages), `event` (what a call tells its host), `turns`
//! (segmentation: detector windows into turns), `recognition` (the queue, the acceptance filter and the merge
//! window), `speech` (a reply in sentence chunks), `playback` (the queue, barge-in and the heard position) and `call`
//! (the state machine that runs the three regions).

#[cfg(native)]
mod audio;
mod call;
mod config;
mod echo;
mod event;
mod io;
mod maybe_send;
mod models;
mod playback;
mod recognition;
mod residency;
mod room;
mod runtime;
mod speech;
mod turns;
mod voice_call;
#[cfg(web)]
mod web;

#[cfg(test)]
mod test_support;

/// The attribute that makes the model interfaces implementable: `#[async_trait]` natively, `#[async_trait(?Send)]`
/// in the wasm32 build.
#[doc(no_inline)]
pub use async_trait::async_trait;
pub use config::{EndOfTurn, ListeningBar, Patience, VoiceConfig};
pub use event::{CallState, Karaoke, Listening, PlaybackState, VoiceError, VoiceEvent};
#[cfg(native)]
pub use io::NativeIo;
pub use io::{AudioIo, IoEvent, IoSink};
pub use maybe_send::{MaybeSend, MaybeSync};
pub use models::{EndOfTurnModel, Models, Speaker, Transcriber, Vad, VadFrame, VoiceModels};
pub use room::{
    Playback, PlaybackReason, PlaybackStatus, Reply, RoomEvent, RoomMessage, TurnPhase,
    TurnTimings, UserTurn,
};
pub use voice_call::{Events, VoiceCall};
