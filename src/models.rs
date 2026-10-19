//! The models a call runs, as interfaces the app implements: a voice activity detector ([`Vad`]) fed as audio
//! arrives, a [`Transcriber`], a [`Speaker`], and, for [`EndOfTurn::Smart`](crate::EndOfTurn::Smart), an
//! [`EndOfTurnModel`]. The app supplies them per half: [`InputModels`] (the detector, the transcriber and the
//! end-of-turn model) and [`OutputModels`] (the speaker), each slot asked for on its own, so that a call loads only
//! what its halves use. Which model fills each slot, and where it runs, is the app's: nothing here names one. Failures
//! are stable codes.
//!
//! [`SharedModels`] is a pool the app may put in front of its loaders: a slot loaded for one call is reused by every
//! other call that asks for it while any holds it, and released once none does.
#![cfg_attr(web, allow(clippy::arc_with_non_send_sync))]
// The public model interfaces use Arc on both targets. Browser models are intentionally local to its event loop.
//!
//! In the wasm32 build the app's models are JavaScript objects with the same methods (`web`, and
//! `js/voice-models.d.ts`).

mod web;

#[cfg(test)]
mod tests;

use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;

use crate::maybe_send::{MaybeSend, MaybeSync};

#[cfg(web)]
pub(crate) use web::{JsInputModels, JsOutputModels};

/// The code of a slot the app does not fill.
pub(crate) const END_OF_TURN_MISSING: &str = "end-of-turn-missing";

/// Where an input half's models come from. Each slot is asked for only when the call needs it: the detector and the
/// transcriber as the input half starts, the end-of-turn model only with [`EndOfTurn::Smart`](crate::EndOfTurn::Smart).
#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
pub trait InputModels: MaybeSend + MaybeSync {
    /// A fresh detector stream, loaded and ready: each call its own (the weights behind it may be shared). The call
    /// warms it up as the other slots load: 0.1 s of silence, then a reset.
    async fn vad(&self) -> Result<Box<dyn Vad>, String>;
    /// The transcriber, loaded and ready.
    async fn transcriber(&self) -> Result<Arc<dyn Transcriber>, String>;
    /// The end-of-turn model, loaded and ready; `end-of-turn-missing` when the app has none.
    async fn end_of_turn(&self) -> Result<Arc<dyn EndOfTurnModel>, String> {
        Err(END_OF_TURN_MISSING.into())
    }
}

/// Where an output half's model comes from.
#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
pub trait OutputModels: MaybeSend + MaybeSync {
    /// The speaker, loaded and ready.
    async fn speaker(&self) -> Result<Arc<dyn Speaker>, String>;
}

/// Creates a fresh detector stream from one loaded detector model. Its weights may be shared, but a stream's
/// recurrent/window state belongs to one call.
#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
pub trait VadFactory: MaybeSend + MaybeSync {
    async fn new_stream(&self) -> Result<Box<dyn Vad>, String>;
}

/// Loads one slot's model for [`SharedModels`], when no call holds the one loaded before.
#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
pub trait SharedModelLoader: MaybeSend + MaybeSync {
    /// The detector model, from which each call gets its own stream.
    async fn vad(&self) -> Result<Arc<dyn VadFactory>, String>;
    async fn transcriber(&self) -> Result<Arc<dyn Transcriber>, String>;
    async fn speaker(&self) -> Result<Arc<dyn Speaker>, String>;
    /// `end-of-turn-missing` when the app has none.
    async fn end_of_turn(&self) -> Result<Arc<dyn EndOfTurnModel>, String> {
        Err(END_OF_TURN_MISSING.into())
    }
}

#[cfg(native)]
type LoadLock = tokio::sync::Mutex<()>;
#[cfg(web)]
type LoadLock = futures_util::lock::Mutex<()>;

/// A pool in front of the app's loader, slot by slot: what one call loaded is reused by every call that asks for the
/// same slot while any call (or a request under way) still holds it, and it is released once none does; the next ask
/// loads it again. It keeps only weak references: the calls' loaded models own them. Requests to a shared transcriber,
/// speaker or end-of-turn model are serialized, so a model with non-reentrant inference is safe. Each call gets its own
/// detector stream from the shared detector model. It is both halves' [`InputModels`] and [`OutputModels`].
pub struct SharedModels {
    loader: Arc<dyn SharedModelLoader>,
    vad: Slot<dyn VadFactory>,
    transcriber: Slot<dyn Transcriber>,
    speaker: Slot<dyn Speaker>,
    end_of_turn: Slot<dyn EndOfTurnModel>,
}

impl SharedModels {
    #[must_use]
    pub fn new(loader: Arc<dyn SharedModelLoader>) -> Self {
        Self {
            loader,
            vad: Slot::default(),
            transcriber: Slot::default(),
            speaker: Slot::default(),
            end_of_turn: Slot::default(),
        }
    }
}

/// One slot of the pool: the model loaded, while anything holds it.
struct Slot<T: ?Sized> {
    cached: Mutex<Weak<Leased<T>>>,
    loading: LoadLock,
}

impl<T: ?Sized> Default for Slot<T> {
    fn default() -> Self {
        Self {
            cached: Mutex::new(Weak::new()),
            loading: LoadLock::new(()),
        }
    }
}

/// A loaded model and the lock its requests take turns on.
struct Leased<T: ?Sized> {
    model: Arc<T>,
    requests: LoadLock,
}

impl<T: ?Sized> Slot<T> {
    /// The model held, or the one `load` gives, held from now by what the answer is kept in.
    async fn get(
        &self,
        load: impl std::future::Future<Output = Result<Arc<T>, String>>,
    ) -> Result<Arc<Leased<T>>, String> {
        let _loading = self.loading.lock().await;
        if let Some(lease) = self.cached.lock().expect("shared model slot").upgrade() {
            return Ok(lease);
        }
        let lease = Arc::new(Leased {
            model: load.await?,
            requests: LoadLock::new(()),
        });
        *self.cached.lock().expect("shared model slot") = Arc::downgrade(&lease);
        Ok(lease)
    }
}

#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
impl InputModels for SharedModels {
    async fn vad(&self) -> Result<Box<dyn Vad>, String> {
        let lease = self.vad.get(self.loader.vad()).await?;
        let stream = lease.model.new_stream().await?;
        Ok(Box::new(LeasedVad {
            stream,
            _lease: lease,
        }))
    }

    async fn transcriber(&self) -> Result<Arc<dyn Transcriber>, String> {
        let lease = self.transcriber.get(self.loader.transcriber()).await?;
        Ok(Arc::new(LeasedTranscriber(lease)))
    }

    async fn end_of_turn(&self) -> Result<Arc<dyn EndOfTurnModel>, String> {
        let lease = self.end_of_turn.get(self.loader.end_of_turn()).await?;
        Ok(Arc::new(LeasedEndOfTurn(lease)))
    }
}

#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
impl OutputModels for SharedModels {
    async fn speaker(&self) -> Result<Arc<dyn Speaker>, String> {
        let lease = self.speaker.get(self.loader.speaker()).await?;
        Ok(Arc::new(LeasedSpeaker(lease)))
    }
}

/// One call's detector stream, holding the shared detector model it came from.
struct LeasedVad {
    stream: Box<dyn Vad>,
    _lease: Arc<Leased<dyn VadFactory>>,
}

struct LeasedTranscriber(Arc<Leased<dyn Transcriber>>);
struct LeasedSpeaker(Arc<Leased<dyn Speaker>>);
struct LeasedEndOfTurn(Arc<Leased<dyn EndOfTurnModel>>);

#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
impl Vad for LeasedVad {
    async fn accept(&mut self, pcm: &[f32]) -> Result<Vec<VadFrame>, String> {
        self.stream.accept(pcm).await
    }

    async fn reset(&mut self) {
        self.stream.reset().await;
    }
}

#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
impl Transcriber for LeasedTranscriber {
    async fn transcribe(
        &self,
        pcm: Vec<f32>,
        sample_rate: u32,
        language: Option<String>,
    ) -> Result<String, String> {
        let _request = self.0.requests.lock().await;
        self.0.model.transcribe(pcm, sample_rate, language).await
    }
}

#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
impl Speaker for LeasedSpeaker {
    async fn speak(
        &self,
        text: String,
        voice: Option<String>,
        language: Option<String>,
        speed: f32,
    ) -> Result<(Vec<f32>, u32), String> {
        let _request = self.0.requests.lock().await;
        self.0.model.speak(text, voice, language, speed).await
    }
}

#[cfg_attr(native, async_trait)]
#[cfg_attr(web, async_trait(?Send))]
impl EndOfTurnModel for LeasedEndOfTurn {
    async fn end_of_turn(&self, pcm: Vec<f32>, sample_rate: u32) -> Result<f32, String> {
        let _request = self.0.requests.lock().await;
        self.0.model.end_of_turn(pcm, sample_rate).await
    }
}

/// What a voice activity detector knows after one of its windows. The call decides on the window's probability,
/// window by window, with its own timing; a frame without one is taken as `speech` says.
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
