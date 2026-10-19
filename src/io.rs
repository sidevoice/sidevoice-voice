//! The microphone and the speaker, as a call uses them ([`AudioIo`]): capture arrives as 16 kHz mono samples, with
//! the echo of the call's own playback already cancelled, and the speaker plays the chunks of what is said in order
//! and says when each starts and ends. The platform's implementations are in `io/`; each file says where it runs.

use std::sync::{Arc, Mutex};

use futures_channel::mpsc::UnboundedSender;

use crate::maybe_send::MaybeSend;
use crate::voice_call::Message;

mod native;

#[cfg(native)]
pub use native::{
    AudioDevice, AudioDevices, InputPort, LostDevices, NativeDevices, NativeIo, SharedNativeAudio,
};

/// What the microphone and the speaker tell the call.
#[derive(Debug, Clone, PartialEq)]
pub enum IoEvent {
    /// What was asked for is open (the speaker, and the microphone when asked): the call listens from now.
    Ready,
    /// Captured audio: 16 kHz mono samples, in order, after echo cancellation.
    Captured(Vec<f32>),
    /// Rendered speaker samples, mono at 16 kHz, after gain and fading.
    Rendered(Vec<f32>),
    /// Playback ended; discard any smoothing tail and report exact silence.
    PlaybackEnded,
    /// The first sample of a chunk reached the speaker.
    ChunkStarted {
        /// What is being said.
        utterance: String,
        /// The chunk's index in it.
        chunk: usize,
    },
    /// The last sample of a chunk reached the speaker.
    ChunkPlayed {
        /// What is being said.
        utterance: String,
        /// The chunk's index in it.
        chunk: usize,
    },
    /// Whether the microphone's device gives no audio for now (muted by the browser or the system, not by the person),
    /// each time that changes.
    MicrophoneMuted(bool),
    /// The microphone failed or went away and no other opened (or it could not open), as a stable code: it is closed,
    /// and the speaker goes on.
    InputLost(String),
    /// The speaker failed, or the devices could not open, as a stable code; the call stops.
    Failed(String),
}

/// Where an [`AudioIo`] sends its [`IoEvent`]s, from any thread.
#[cfg_attr(web, wasm_bindgen::prelude::wasm_bindgen)]
#[derive(Debug, Clone)]
pub struct IoSink {
    pub(crate) messages: UnboundedSender<Message>,
    pub(crate) generation: u64,
    pub(crate) rendered: Arc<Mutex<RenderedMailbox>>,
    #[cfg_attr(web, allow(dead_code))]
    pub(crate) direct_rendered: bool,
}

/// Only the newest rendered frame waits while the call task is occupied. A replacement marks a discontinuity so
/// the driver does not splice an old envelope to the new frame.
#[derive(Debug, Default)]
pub(crate) struct RenderedMailbox {
    generation: u64,
    queued: bool,
    latest: Option<RenderedFrame>,
}

#[derive(Debug)]
pub(crate) struct RenderedFrame {
    pub(crate) samples: Vec<f32>,
    pub(crate) skipped: bool,
    pub(crate) at_ms: u64,
}

impl RenderedMailbox {
    pub(crate) fn advance(&mut self, generation: u64) {
        if generation > self.generation {
            self.generation = generation;
            self.queued = false;
            self.latest = None;
        }
    }

    pub(crate) fn take(&mut self, generation: u64) -> Option<RenderedFrame> {
        if generation != self.generation {
            return None;
        }
        self.queued = false;
        self.latest.take()
    }
}

impl IoSink {
    /// Sends one event; once the call is gone, it goes nowhere.
    pub fn send(&self, event: IoEvent) {
        match event {
            IoEvent::Rendered(samples) => {
                #[cfg(native)]
                if self.direct_rendered {
                    let _ = self.messages.unbounded_send(Message::PhysicalRendered(
                        self.generation,
                        vec![true; samples.len()],
                        samples,
                    ));
                    return;
                }
                let mut mailbox = self.rendered.lock().expect("rendered mailbox");
                if self.generation < mailbox.generation {
                    return;
                }
                mailbox.advance(self.generation);
                let skipped = mailbox.latest.is_some();
                mailbox.latest = Some(RenderedFrame {
                    samples,
                    skipped,
                    at_ms: crate::runtime::monotonic_ms(),
                });
                if !mailbox.queued {
                    mailbox.queued = true;
                    if self
                        .messages
                        .unbounded_send(Message::Rendered(self.generation))
                        .is_err()
                    {
                        mailbox.queued = false;
                        mailbox.latest = None;
                    }
                }
            }
            other => {
                let _ = self
                    .messages
                    .unbounded_send(Message::Io(self.generation, other));
            }
        }
    }

    /// The native callback's rendered samples and its aligned queued-playback markers. Private to the physical
    /// backend: ordinary AudioIo implementers retain the public Rendered event.
    #[cfg(native)]
    pub(crate) fn send_physical_rendered(&self, samples: Vec<f32>, queued: Vec<bool>) {
        debug_assert_eq!(samples.len(), queued.len());
        if self.direct_rendered {
            let _ = self.messages.unbounded_send(Message::PhysicalRendered(
                self.generation,
                queued,
                samples,
            ));
        } else {
            self.send(IoEvent::Rendered(samples));
        }
    }
}

/// A microphone and a speaker for one call.
pub trait AudioIo: MaybeSend {
    /// Starts opening the speaker and, with `input`, the microphone; every event goes to `sink`. Without `input` no
    /// microphone is asked for (a call that only speaks). Once what was asked for works it sends [`IoEvent::Ready`],
    /// or [`IoEvent::Failed`] if it cannot; the call listens only from `Ready`. Fails at once with a stable code when
    /// it cannot even begin.
    ///
    /// # Errors
    ///
    /// The code of why the microphone or the speaker could not be opened.
    fn start(&mut self, sink: IoSink, input: bool) -> Result<(), String>;
    /// Opens the microphone (`true`) or closes it (`false`) while the speaker goes on, from the next frame on. One that
    /// cannot open is said as [`IoEvent::InputLost`]. Before `start`, or after `stop`, it does nothing: `start` says
    /// whether to open it.
    fn set_input(&mut self, input: bool);
    /// Queues a chunk of something said: mono samples at `sample_rate`, played after whatever is queued.
    fn play(&mut self, utterance: &str, chunk: usize, samples: Vec<f32>, sample_rate: u32);
    /// Stops the speaker at once, with a short fade, and drops every queued chunk.
    fn stop_playback(&mut self);
    /// Stops capturing and closes the speaker.
    fn stop(&mut self);
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{IoEvent, IoSink, RenderedMailbox};
    use crate::voice_call::Message;
    use futures_channel::mpsc::unbounded;

    #[test]
    fn stalled_consumer_gets_latest_silence_and_a_discontinuity() {
        let (messages, mut received) = unbounded();
        let mailbox = Arc::new(Mutex::new(RenderedMailbox::default()));
        let sink = IoSink {
            messages,
            generation: 1,
            rendered: Arc::clone(&mailbox),
            direct_rendered: false,
        };
        for _ in 0..100 {
            sink.send(IoEvent::Rendered(vec![0.8; 160]));
        }
        sink.send(IoEvent::Rendered(vec![0.0; 160]));
        assert!(matches!(received.try_recv(), Ok(Message::Rendered(1))));
        let latest = mailbox.lock().unwrap().take(1).unwrap();
        assert!(latest.skipped);
        assert!(latest.samples.iter().all(|sample| *sample == 0.0));
        assert!(received.try_recv().is_err());
        mailbox.lock().unwrap().advance(2);
        sink.send(IoEvent::Rendered(vec![0.8; 160]));
        assert!(mailbox.lock().unwrap().take(2).is_none());
    }
}
