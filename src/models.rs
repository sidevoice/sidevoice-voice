//! The models a call runs, as the driver uses them: a voice activity detector it feeds as audio arrives, a
//! transcriber and a speaker. Every model is the engine's; `models/` holds the bridge of each build to it (natively
//! the engine crate itself, in the wasm32 build the page's `WebEngine`), and each file says where it runs. Failures
//! are the engine's stable codes.

mod engine;
mod web;

use std::sync::Arc;

use async_trait::async_trait;

use crate::config::VoiceConfig;
use crate::maybe_send::{MaybeSend, MaybeSync};

#[cfg(native)]
pub(crate) use engine::EngineModels;
#[cfg(web)]
pub(crate) use web::WebModels;

/// How the call's detector decides what is speech: a probability of 0.6, speech confirmed after 400 ms, and ended
/// after 200 ms below it (the end of a turn waits the patience's silence on top).
pub(crate) const VAD_THRESHOLD: f32 = 0.6;
pub(crate) const VAD_MIN_SPEECH_MS: u32 = 400;
pub(crate) const VAD_MIN_SILENCE_MS: u32 = 200;

/// Where the models come from.
#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
pub(crate) trait Models: MaybeSend + MaybeSync {
    /// Loads the configuration's three stages, installing them first if they are not.
    async fn load(&self, config: &VoiceConfig) -> Result<Loaded, String>;
}

/// The three stages, loaded.
pub(crate) struct Loaded {
    pub(crate) detector: Box<dyn Detector>,
    pub(crate) transcriber: Arc<dyn Transcriber>,
    pub(crate) speaker: Arc<dyn Speaker>,
}

/// A voice activity detector's stream over the 16 kHz capture.
#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
pub(crate) trait Detector: MaybeSend {
    /// Takes the next samples, of any length, and returns one `(end, speech)` per whole window they completed: the
    /// sample after the window's last, counted from the stream's start or last reset, and whether the stream is
    /// inside speech after it.
    async fn accept(&mut self, pcm: &[f32]) -> Result<Vec<(u64, bool)>, String>;
    /// Starts over from sample 0, with no speech.
    async fn reset(&mut self);
}

/// Speech to text.
#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
pub(crate) trait Transcriber: MaybeSend + MaybeSync {
    /// The text of 16 kHz mono `pcm`, in `language` (`None`: detected).
    async fn transcribe(&self, pcm: Vec<f32>, language: Option<String>) -> Result<String, String>;
}

/// Text to speech.
#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
pub(crate) trait Speaker: MaybeSend + MaybeSync {
    /// `text` spoken in `language`, as mono samples and their rate.
    async fn speak(
        &self,
        text: String,
        language: Option<String>,
    ) -> Result<(Vec<f32>, u32), String>;
}
