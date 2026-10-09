//! [`VoiceCall`]: one call, run as a task of its own around the state machine (`call`). The task loads the
//! configuration's models, opens the microphone and the speaker, feeds the capture to the detector and its windows to
//! the state machine, runs each transcription and each synthesis as a task of its own (so a long one never holds the
//! detector back), and does what the state machine answers. Everything reaches it as messages on one channel; the
//! host hears it through [`Events`].

#[cfg(all(test, native))]
mod tests;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures_channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures_util::future::{select, Either};
use futures_util::StreamExt;

use crate::call::{Call, Effect, Input, Transcript};
use crate::config::{EndOfTurn, VoiceConfig};
use crate::event::{VoiceError, VoiceEvent};
use crate::io::{AudioIo, IoEvent, IoSink};
use crate::models::{Loaded, Models};
use crate::residency::Residency;
use crate::room::RoomEvent;
use crate::runtime::{monotonic_ms, sleep, spawn, unix_ms};

/// What a call tells its host, in order.
pub type Events = UnboundedReceiver<VoiceEvent>;

/// What reaches the call's task.
pub(crate) enum Message {
    Start,
    Stop,
    Config(Box<VoiceConfig>),
    Room(RoomEvent),
    Online(bool),
    Mute(bool),
    CancelInput,
    Io(IoEvent),
    Transcribed {
        turn: usize,
        result: Result<String, String>,
    },
    Synthesized {
        utterance: String,
        chunk: usize,
        result: Result<(Vec<f32>, u32), String>,
    },
}

/// One voice call. Its methods only send a message to the call's task and return; what comes of them arrives
/// through the [`Events`] it was created with. Dropping it ends the call.
#[derive(Debug)]
pub struct VoiceCall {
    messages: UnboundedSender<Message>,
}

impl VoiceCall {
    /// A call on `engine`'s models, the microphone and speaker of `io`, set up with `config`. It does nothing until
    /// [`VoiceCall::start`]. Its task runs on the app's Tokio runtime, which the engine needs too.
    #[cfg(native)]
    #[must_use]
    pub fn new(
        engine: Arc<sidevoice_engine::Engine>,
        io: Box<dyn AudioIo>,
        config: VoiceConfig,
    ) -> (Self, Events) {
        Self::with_models(Arc::new(crate::models::EngineModels(engine)), io, config)
    }

    pub(crate) fn with_models(
        models: Arc<dyn Models>,
        io: Box<dyn AudioIo>,
        config: VoiceConfig,
    ) -> (Self, Events) {
        let (messages, inbox) = unbounded();
        let (events, received) = unbounded();
        let driver = Driver {
            call: Call::new(
                config.clone(),
                call_id(),
                unix_ms().saturating_sub(monotonic_ms()),
            ),
            residency: Residency::new(config.idle_unload_minutes),
            config,
            models,
            io,
            loaded: None,
            running: false,
            captured: VecDeque::new(),
            taken: 0,
            messages: messages.clone(),
            events,
        };
        spawn(driver.run(inbox));
        (Self { messages }, received)
    }

    /// Loads the models (installing them first if they are not: install them through the engine beforehand to show
    /// the progress), opens the microphone and the speaker, and starts listening.
    pub fn start(&self) {
        self.send(Message::Start);
    }

    /// Stops listening and speaking: the person's turn not yet reported is cancelled, the reply playing is
    /// interrupted and the queued ones are dropped. The models stay loaded for the next start.
    pub fn stop(&self) {
        self.send(Message::Stop);
    }

    /// A new configuration. Changed stages are loaded again at once if the call is started, else at the next start.
    pub fn set_config(&self, config: VoiceConfig) {
        self.send(Message::Config(Box::new(config)));
    }

    /// A message the room sent.
    pub fn room_event(&self, event: RoomEvent) {
        self.send(Message::Room(event));
    }

    /// Whether the room is in reach: turns reported while it is not say `offline`.
    pub fn set_online(&self, online: bool) {
        self.send(Message::Online(online));
    }

    /// Mutes or unmutes the microphone; muting ends the open turn with what was said.
    pub fn mute(&self, muted: bool) {
        self.send(Message::Mute(muted));
    }

    /// Cancels what the person said that is not reported yet.
    pub fn cancel_input(&self) {
        self.send(Message::CancelInput);
    }

    fn send(&self, message: Message) {
        let _ = self.messages.unbounded_send(message);
    }
}

/// The call's task.
struct Driver {
    call: Call,
    config: VoiceConfig,
    models: Arc<dyn Models>,
    io: Box<dyn AudioIo>,
    loaded: Option<Loaded>,
    /// When the loaded models leave memory, the call stopped.
    residency: Residency,
    running: bool,
    /// Capture not yet in a whole detector window.
    captured: VecDeque<f32>,
    /// The detector's position: samples taken into windows since its last reset.
    taken: u64,
    messages: UnboundedSender<Message>,
    events: UnboundedSender<VoiceEvent>,
}

impl Driver {
    async fn run(mut self, mut inbox: UnboundedReceiver<Message>) {
        loop {
            let deadline = [self.call.deadline(), self.residency.deadline()]
                .into_iter()
                .flatten()
                .min();
            let message = match deadline {
                None => inbox.next().await,
                Some(deadline) => {
                    let wait = deadline.saturating_sub(monotonic_ms());
                    match select(inbox.next(), Box::pin(sleep(wait))).await {
                        Either::Left((message, _)) => message,
                        Either::Right(((), _)) => {
                            let now = monotonic_ms();
                            let effects = self.call.poll(now);
                            self.apply(effects);
                            if self.residency.due(now) {
                                // Dropping them is what unloads them; the next start loads them again.
                                self.loaded = None;
                            }
                            continue;
                        }
                    }
                }
            };
            let Some(message) = message else {
                // The VoiceCall was dropped.
                self.halt().await;
                return;
            };
            self.receive(message).await;
            let idle = self.loaded.is_some() && !self.running;
            self.residency.observe(monotonic_ms(), idle);
        }
    }

    async fn receive(&mut self, message: Message) {
        match message {
            Message::Start => self.start().await,
            Message::Stop => self.halt().await,
            Message::Config(config) => {
                let reload = self.config.vad != config.vad
                    || self.config.stt != config.stt
                    || self.config.tts != config.tts;
                self.config = (*config).clone();
                self.residency.set_minutes(config.idle_unload_minutes);
                self.input(Input::Config(config));
                if reload {
                    self.loaded = None;
                    if self.running {
                        self.halt().await;
                        self.start().await;
                    }
                }
            }
            Message::Room(event) => self.input(Input::Room(event)),
            Message::Online(online) => self.input(Input::Online(online)),
            Message::Mute(muted) => self.input(Input::Mute(muted)),
            Message::CancelInput => self.input(Input::Cancel),
            Message::Io(IoEvent::Captured(pcm)) => self.captured(&pcm).await,
            Message::Io(IoEvent::ChunkStarted { utterance, chunk }) => {
                self.input(Input::ChunkStarted { utterance, chunk });
            }
            Message::Io(IoEvent::ChunkPlayed { utterance, chunk }) => {
                self.input(Input::ChunkPlayed { utterance, chunk });
            }
            Message::Io(IoEvent::Failed(code)) => {
                self.error(code);
                self.halt().await;
            }
            Message::Transcribed { turn, result } => self.input(Input::Transcribed {
                turn,
                result: result.map(|text| Transcript {
                    text,
                    logprob: None,
                }),
            }),
            Message::Synthesized {
                utterance,
                chunk,
                result,
            } => self.input(Input::Synthesized {
                utterance,
                chunk,
                result,
            }),
        }
    }

    async fn start(&mut self) {
        if self.running {
            return;
        }
        if self.config.end_of_turn == EndOfTurn::SmartTurn {
            self.error("end-of-turn-unavailable".into());
            return;
        }
        if self.loaded.is_none() {
            match self.models.load(&self.config).await {
                Ok(loaded) => self.loaded = Some(loaded),
                Err(code) => {
                    self.error(code);
                    return;
                }
            }
        }
        let sink = IoSink(self.messages.clone());
        if let Err(code) = self.io.start(sink) {
            self.error(code);
            return;
        }
        self.running = true;
        self.input(Input::Start);
    }

    /// Stops the call, its microphone and its speaker, and starts the detector over.
    async fn halt(&mut self) {
        if !self.running {
            return;
        }
        self.running = false;
        self.input(Input::Stop);
        self.io.stop();
        self.captured.clear();
        self.taken = 0;
        if let Some(loaded) = &mut self.loaded {
            loaded.detector.reset().await;
        }
    }

    /// Runs the detector on new capture and hands each window it completed to the state machine.
    async fn captured(&mut self, pcm: &[f32]) {
        let Some(loaded) = &mut self.loaded else {
            return;
        };
        if !self.running {
            return;
        }
        self.captured.extend(pcm);
        match loaded.detector.accept(pcm).await {
            Ok(windows) => {
                for (end, speech) in windows {
                    let take = (end.saturating_sub(self.taken) as usize).min(self.captured.len());
                    self.taken = end;
                    let window: Vec<f32> = self.captured.drain(..take).collect();
                    self.input(Input::Window {
                        pcm: window,
                        speech,
                    });
                }
            }
            Err(code) => {
                loaded.detector.reset().await;
                self.captured.clear();
                self.taken = 0;
                self.error(code);
            }
        }
    }

    fn input(&mut self, input: Input) {
        let effects = self.call.handle(monotonic_ms(), input);
        self.apply(effects);
    }

    fn apply(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Event(event) => {
                    let _ = self.events.unbounded_send(event);
                }
                Effect::Transcribe {
                    turn,
                    pcm,
                    language,
                } => {
                    let Some(loaded) = &self.loaded else { continue };
                    let transcriber = Arc::clone(&loaded.transcriber);
                    let messages = self.messages.clone();
                    spawn(async move {
                        let result = transcriber.transcribe(pcm, language).await;
                        let _ = messages.unbounded_send(Message::Transcribed { turn, result });
                    });
                }
                Effect::Synthesize {
                    utterance,
                    chunk,
                    text,
                    language,
                } => {
                    let Some(loaded) = &self.loaded else { continue };
                    let speaker = Arc::clone(&loaded.speaker);
                    let messages = self.messages.clone();
                    spawn(async move {
                        let result = speaker.speak(text, language).await;
                        let _ = messages.unbounded_send(Message::Synthesized {
                            utterance,
                            chunk,
                            result,
                        });
                    });
                }
                Effect::Play {
                    utterance,
                    chunk,
                    samples,
                    sample_rate,
                } => self.io.play(&utterance, chunk, samples, sample_rate),
                Effect::StopPlayback => self.io.stop_playback(),
            }
        }
    }

    fn error(&self, code: String) {
        let _ = self
            .events
            .unbounded_send(VoiceEvent::Error(VoiceError { code }));
    }
}

/// An id for a call, unique on this device: the time it was made and a counter, in hexadecimal.
fn call_id() -> String {
    static CALLS: AtomicU64 = AtomicU64::new(0);
    format!("{:x}{:x}", unix_ms(), CALLS.fetch_add(1, Ordering::Relaxed))
}
