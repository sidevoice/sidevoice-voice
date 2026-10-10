//! The voice call on the device: the microphone heard as turns of speech, transcribed, and told to the app as words;
//! what the app asks it to say spoken, played, cut when the person speaks over it, and told back as heard, heard up to
//! a point, or not played. The models it runs are the app's, through the module's own interfaces (`models`:
//! [`VoiceModels`], [`Vad`], [`Transcriber`], [`Speaker`], [`EndOfTurnModel`]); nothing here names or links one. It
//! knows nothing of what the app does with the words or where what it says comes from.
//!
//! Inside: `config` ([`VoiceConfig`]), `event` (what a call tells its host), `say` (something said and its handle),
//! `turns` (segmentation: detector windows into turns), `recognition` (the queue, the acceptance filter and the merge
//! window), `speech` (a text in sentence chunks), `playback` (the queue, barge-in and the heard position) and `call`
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
mod runtime;
mod say;
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
pub use event::{
    CallState, Listening, PlaybackState, TurnEvent, TurnTimings, VoiceError, VoiceEvent,
};
#[cfg(native)]
pub use io::NativeIo;
pub use io::{AudioIo, IoEvent, IoSink};
pub use maybe_send::{MaybeSend, MaybeSync};
pub use models::{EndOfTurnModel, Models, Speaker, Transcriber, Vad, VadFrame, VoiceModels};
pub use say::{SayCancel, SayEvent, SayOptions, SayOutcome, Saying, StopReason};
pub use voice_call::{Events, VoiceCall};
