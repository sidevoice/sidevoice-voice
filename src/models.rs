//! The models a call runs, as interfaces the app implements: a voice activity detector ([`Vad`]) fed as audio
//! arrives, a [`Transcriber`], a [`Speaker`], and, for `smart-turn`, an [`EndOfTurnModel`]. The app supplies them
//! through [`VoiceModels`], which the call asks to load on start and drops once the call has been stopped for its idle
//! minutes. Which model fills each slot, and where it runs, is the app's: nothing here names one. Failures are stable
//! codes.
//!
//! In the wasm32 build the app's models are JavaScript objects with the same methods (`web`, and
//! `js/voice-models.d.ts`).

mod web;

use std::sync::Arc;

use async_trait::async_trait;

use crate::maybe_send::{MaybeSend, MaybeSync};

#[cfg(web)]
pub(crate) use web::JsVoiceModels;

/// Where a call's models come from: loaded when the call starts, dropped when it has been stopped for a while.
#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
pub trait VoiceModels: MaybeSend + MaybeSync {
    /// The models, loaded and ready; a stable code when one cannot be.
    async fn load(&self) -> Result<Models, String>;
}

/// The models of one call.
pub struct Models {
    /// The voice activity detector, on the call's 16 kHz mono capture.
    pub vad: Box<dyn Vad>,
    pub transcriber: Arc<dyn Transcriber>,
    pub speaker: Arc<dyn Speaker>,
    /// What `smart-turn` asks at each pause; `None` leaves the call to `silence`.
    pub end_of_turn: Option<Arc<dyn EndOfTurnModel>>,
}

/// What a voice activity detector knows after one of its windows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VadFrame {
    /// The sample after the window's last, counted from the stream's start or its last reset.
    pub end: u64,
    /// Whether speech is going on after it: confirmed, and not yet over.
    pub speech: bool,
    /// The model's probability of speech for the window, when it gives one.
    pub probability: Option<f32>,
}

/// A voice activity detector's stream over 16 kHz mono audio. Speech starts and ends where `speech` changes.
#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
pub trait Vad: MaybeSend {
    /// Takes the next samples, of any length, and returns one frame per whole window they completed, in order.
    async fn accept(&mut self, pcm: &[f32]) -> Result<Vec<VadFrame>, String>;
    /// Starts over from sample 0, with no speech.
    async fn reset(&mut self);
}

/// Speech to text.
#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
pub trait Transcriber: MaybeSend + MaybeSync {
    /// The text of mono `pcm` at `sample_rate`, in `language` (a BCP 47 tag; `None`: detected).
    async fn transcribe(
        &self,
        pcm: Vec<f32>,
        sample_rate: u32,
        language: Option<String>,
    ) -> Result<String, String>;
}

/// Text to speech.
#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
pub trait Speaker: MaybeSend + MaybeSync {
    /// `text` spoken with `voice` (the speaker's default when `None`), in `language`, at `speed` (1.0 its own): mono
    /// samples and their rate.
    async fn speak(
        &self,
        text: String,
        voice: Option<String>,
        language: Option<String>,
        speed: f32,
    ) -> Result<(Vec<f32>, u32), String>;
}

/// An end-of-turn classifier: whether a turn's audio so far sounds finished.
#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
pub trait EndOfTurnModel: MaybeSend + MaybeSync {
    /// The probability, from 0 to 1, that the speaker of mono `pcm` at `sample_rate` has finished their turn.
    async fn end_of_turn(&self, pcm: Vec<f32>, sample_rate: u32) -> Result<f32, String>;
}

impl Models {
    /// Models without an end-of-turn classifier.
    #[must_use]
    pub fn new(
        vad: Box<dyn Vad>,
        transcriber: Arc<dyn Transcriber>,
        speaker: Arc<dyn Speaker>,
    ) -> Self {
        Self {
            vad,
            transcriber,
            speaker,
            end_of_turn: None,
        }
    }
}
