//! The microphone and the speaker, as a call uses them ([`AudioIo`]): capture arrives as 16 kHz mono samples, with
//! the echo of the call's own playback already cancelled, and the speaker plays a reply's chunks in order and says
//! when each starts and ends. The platform's implementations are in `io/`; each file says where it runs.

use futures_channel::mpsc::UnboundedSender;

use crate::maybe_send::MaybeSend;
use crate::voice_call::Message;

mod native;

#[cfg(native)]
pub use native::NativeIo;

/// What the microphone and the speaker tell the call.
#[derive(Debug, Clone, PartialEq)]
pub enum IoEvent {
    /// Captured audio: 16 kHz mono samples, in order, after echo cancellation.
    Captured(Vec<f32>),
    /// The first sample of a chunk reached the speaker.
    ChunkStarted {
        /// The reply.
        utterance: String,
        /// The chunk's index in it.
        chunk: usize,
    },
    /// The last sample of a chunk reached the speaker.
    ChunkPlayed {
        /// The reply.
        utterance: String,
        /// The chunk's index in it.
        chunk: usize,
    },
    /// The microphone or the speaker failed, as a stable code; the call stops.
    Failed(String),
}

/// Where an [`AudioIo`] sends its [`IoEvent`]s, from any thread.
#[cfg_attr(web, wasm_bindgen::prelude::wasm_bindgen)]
#[derive(Debug, Clone)]
pub struct IoSink(pub(crate) UnboundedSender<Message>);

impl IoSink {
    /// Sends one event; once the call is gone, it goes nowhere.
    pub fn send(&self, event: IoEvent) {
        let _ = self.0.unbounded_send(Message::Io(event));
    }
}

/// A microphone and a speaker for one call.
pub trait AudioIo: MaybeSend {
    /// Starts capturing and opens the speaker; every event goes to `sink`. Fails with a stable code.
    ///
    /// # Errors
    ///
    /// The code of why the microphone or the speaker could not be opened.
    fn start(&mut self, sink: IoSink) -> Result<(), String>;
    /// Queues a chunk of a reply: mono samples at `sample_rate`, played after whatever is queued.
    fn play(&mut self, utterance: &str, chunk: usize, samples: Vec<f32>, sample_rate: u32);
    /// Stops the speaker at once, with a short fade, and drops every queued chunk.
    fn stop_playback(&mut self);
    /// Stops capturing and closes the speaker.
    fn stop(&mut self);
}
