//! The microphone and the speaker, as a call uses them ([`AudioIo`]): capture arrives as 16 kHz mono samples, with
//! the echo of the call's own playback already cancelled, and the speaker plays the chunks of what is said in order
//! and says when each starts and ends. The platform's implementations are in `io/`; each file says where it runs.

use std::sync::{Arc, Mutex};

use futures_channel::mpsc::UnboundedSender;

use crate::event::PlaybackCapture;
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
    /// The microphone and the speaker are open: the call listens from now.
    Ready,
    /// Captured audio: 16 kHz mono samples, in order, after echo cancellation.
    Captured(Vec<f32>),
    /// Browser AudioContext frame of this block's first microphone sample.
    #[cfg(web)]
    CapturedSource(Vec<f32>, u64),
    /// Native shared audio preserves the physical capture callback's source time across routing.
    #[cfg(native)]
    CapturedAt(Vec<f32>, u64),
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
    /// A browser source actually ended; its planned final frame shares the capture worklet clock.
    #[cfg(web)]
    ChunkPlayedAt {
        utterance: String,
        chunk: usize,
        frame: u64,
    },
    /// Whether the microphone's device gives no audio for now (muted by the browser or the system, not by the person),
    /// each time that changes.
    MicrophoneMuted(bool),
    /// The microphone or the speaker failed, as a stable code; the call stops.
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
    /// A prior capture block's immutable speaker state, shared with the call actor.
    pub(crate) capture: Option<Arc<Mutex<CaptureEvidence>>>,
}

#[derive(Debug)]
pub(crate) struct CaptureEvidence {
    pub(crate) published: Arc<PlaybackCapture>,
    previous: Option<Arc<PlaybackCapture>>,
}

impl CaptureEvidence {
    pub(crate) fn new(initial: PlaybackCapture) -> Self {
        let initial = Arc::new(initial);
        Self {
            published: initial,
            previous: None,
        }
    }

    fn ingress(&mut self) -> Arc<PlaybackCapture> {
        let before = self
            .previous
            .clone()
            .unwrap_or_else(|| Arc::new(PlaybackCapture::unknown()));
        self.previous = Some(Arc::clone(&self.published));
        before
    }

    pub(crate) fn reset(&mut self) {
        self.published = Arc::new(PlaybackCapture::unknown());
        self.previous = None;
    }
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
            IoEvent::Captured(samples) => {
                let at = crate::runtime::unix_ms()
                    .saturating_sub(crate::turns::duration_ms(samples.len()));
                self.send_capture(samples, at, None);
            }
            #[cfg(web)]
            IoEvent::CapturedSource(samples, frame) => {
                let at = crate::runtime::unix_ms()
                    .saturating_sub(crate::turns::duration_ms(samples.len()));
                self.send_capture(samples, at, Some(frame));
            }
            #[cfg(native)]
            IoEvent::CapturedAt(samples, at) => self.send_capture(samples, at, None),
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

    fn send_capture(&self, samples: Vec<f32>, at: u64, source_frame: Option<u64>) {
        if let Some(capture) = &self.capture {
            let evidence = capture.lock().expect("capture evidence").ingress();
            let _ = self.messages.unbounded_send(Message::Captured(
                self.generation,
                samples,
                evidence,
                at,
                source_frame,
            ));
        } else {
            #[cfg(native)]
            if self.direct_rendered {
                let _ = self.messages.unbounded_send(Message::Io(
                    self.generation,
                    IoEvent::CapturedAt(samples, at),
                ));
                return;
            }
            let _ = self
                .messages
                .unbounded_send(Message::Io(self.generation, IoEvent::Captured(samples)));
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
    /// Starts opening the microphone and the speaker; every event goes to `sink`. Once capture and playback work it
    /// sends [`IoEvent::Ready`], or [`IoEvent::Failed`] if they cannot; the call listens only from `Ready`. Fails at
    /// once with a stable code when it cannot even begin.
    ///
    /// # Errors
    ///
    /// The code of why the microphone or the speaker could not be opened.
    fn start(&mut self, sink: IoSink) -> Result<(), String>;
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

    use super::{CaptureEvidence, IoEvent, IoSink, RenderedMailbox};
    use crate::event::{CaptureHearing, PlaybackCapture, SayCapture};
    use crate::voice_call::Message;
    use futures_channel::mpsc::unbounded;

    #[test]
    fn ingress_keeps_a_prior_block_bound_when_actor_or_vad_is_late() {
        let mut source = CaptureEvidence::new(PlaybackCapture {
            available: true,
            mark: 1,
            recent: vec![],
            last_played: None,
            truncated: false,
        });
        let first = source.ingress();
        assert!(!first.available);
        // A speaker completion processed between microphone callbacks must not
        // be claimed for the next block: its first speech sample may precede it.
        source.published = Arc::new(PlaybackCapture {
            available: true,
            mark: 1,
            recent: vec![SayCapture {
                say_id: "A".into(),
                hearing: CaptureHearing::Complete,
                heard_chars: Some(3),
                played_order: Some(1),
            }],
            last_played: None,
            truncated: false,
        });
        let second = source.ingress();
        assert!(second.available);
        assert!(second.recent.is_empty());
        let third = source.ingress();
        assert_eq!(third.recent[0].heard_chars, Some(3));
        assert!(second.recent.is_empty()); // immutable across delayed VAD
    }

    #[test]
    fn stalled_consumer_gets_latest_silence_and_a_discontinuity() {
        let (messages, mut received) = unbounded();
        let mailbox = Arc::new(Mutex::new(RenderedMailbox::default()));
        let sink = IoSink {
            messages,
            generation: 1,
            rendered: Arc::clone(&mailbox),
            direct_rendered: false,
            capture: None,
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
