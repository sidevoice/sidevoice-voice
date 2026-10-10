//! [`VoiceCall`]: one call, run as a task of its own around the state machine (`call`). The task loads the app's
//! models ([`VoiceModels`]) as the call starts and drops them once it has been stopped for its idle minutes, opens
//! the microphone and the speaker, feeds the capture to the detector and its windows to the state machine, runs each
//! transcription, synthesis and end-of-turn question as a task of its own (so a long one never holds the detector
//! back), and does what the state machine answers. The owner's calls reach it on a channel only [`VoiceCall`] holds,
//! and what the microphone, the speaker and the model tasks report on one of its own; the host hears it through
//! [`Events`]. When the owner's channel closes (the [`VoiceCall`] was dropped) the task stops the call, drops the
//! models and the microphone and speaker, and ends: what its tasks report after that goes nowhere.

#[cfg(all(test, native))]
mod tests;

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures_channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures_util::future::{abortable, ready, select, AbortHandle, Either};
use futures_util::{stream, Stream, StreamExt};

use crate::call::{Call, Effect, Input, Transcript};
use crate::config::{EndOfTurn, VoiceConfig};
use crate::event::{VoiceError, VoiceEvent};
use crate::io::{AudioIo, IoEvent, IoSink};
use crate::maybe_send::MaybeSend;
use crate::models::{Models, VadFrame, VoiceModels};
use crate::residency::Residency;
use crate::runtime::{monotonic_ms, sleep, spawn, unix_ms};
use crate::say::{SayEvent, SayOptions, Saying};
use crate::turns::RATE;

/// How long a stop, or the owner gone, waits behind a message the task is handling (a model that does not answer)
/// before that message is abandoned.
const ABANDON_AFTER_MS: u64 = 500;
/// How many model tasks are kept to abort; older ones have ended.
const TASKS_KEPT: usize = 64;

/// How handling one message ended.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Settled {
    Done,
    /// A stop waited behind it too long.
    Stopped,
    /// The owner is gone.
    Gone,
}

/// What a call tells its host, in order.
pub type Events = UnboundedReceiver<VoiceEvent>;

/// What reaches the call's task.
pub(crate) enum Message {
    Start,
    Stop,
    Config(Box<VoiceConfig>),
    /// Say `text`, telling `events` what becomes of it.
    Say {
        id: String,
        text: String,
        options: SayOptions,
        events: UnboundedSender<SayEvent>,
    },
    /// Cancel what is said under this id.
    CancelSay(String),
    Mute(bool),
    CancelInput,
    Models(Arc<dyn VoiceModels>, Box<VoiceConfig>),
    Io(IoEvent),
    EndOfTurn {
        turn: usize,
        pause: u32,
        result: Result<f32, String>,
    },
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
/// through the [`Events`] it was created with, and what it says through its handle. Dropping it ends the call.
#[derive(Debug)]
pub struct VoiceCall {
    messages: UnboundedSender<Message>,
    /// The task's own channel, which the handles of what it says cancel through: it outlives the owner's.
    internal: UnboundedSender<Message>,
    /// The prefix of the ids the call makes, and how many things it was asked to say.
    call_id: String,
    said: AtomicU64,
}

impl VoiceCall {
    /// A call on the app's `models`, the microphone and speaker of `io`, set up with `config`. It does nothing until
    /// [`VoiceCall::start`]. Natively its task runs on the app's Tokio runtime.
    #[must_use]
    pub fn new(
        models: Arc<dyn VoiceModels>,
        io: Box<dyn AudioIo>,
        config: VoiceConfig,
    ) -> (Self, Events) {
        let (messages, inbox) = unbounded();
        let (internal, reported) = unbounded();
        let (events, received) = unbounded();
        let call_id = call_id();
        let driver = Driver {
            call: Call::new(
                config.clone(),
                call_id.clone(),
                unix_ms().saturating_sub(monotonic_ms()),
            ),
            residency: Residency::new(config.idle_unload_minutes),
            config,
            models,
            io,
            loaded: None,
            running: false,
            opening: false,
            tasks: VecDeque::new(),
            captured: VecDeque::new(),
            taken: 0,
            messages: internal.clone(),
            events,
            sayers: HashMap::new(),
        };
        spawn(driver.run(inbox, reported));
        let call = Self {
            messages,
            internal,
            call_id,
            said: AtomicU64::new(0),
        };
        (call, received)
    }

    /// Loads the models if they are not, opens the microphone and the speaker, and starts listening.
    pub fn start(&self) {
        self.send(Message::Start);
    }

    /// Stops listening and speaking: the person's turn not yet reported is cancelled, and what is being said or
    /// waits to be is not (`stopped`). The models stay loaded for the next start.
    pub fn stop(&self) {
        self.send(Message::Stop);
    }

    /// A new configuration, in effect at once.
    pub fn set_config(&self, config: VoiceConfig) {
        self.send(Message::Config(Box::new(config)));
    }

    /// Other models, with the configuration they go with, taken together: the models loaded are dropped, and the new
    /// ones loaded at once if the call is started (it restarts once, on both: the open turn is cancelled and what is being
    /// said stops), else at the next start. A pair the call cannot run (`smart-turn` without an end-of-turn
    /// model) stops it with `end-of-turn-missing`.
    pub fn set_models(&self, models: Arc<dyn VoiceModels>, config: VoiceConfig) {
        self.send(Message::Models(models, Box::new(config)));
    }

    /// Says `text` after whatever is being said, once nothing of the person's holds it back (an open turn, one being
    /// transcribed, the grace after one). The handle tells, in order, when it sounds, where the reader is and how it
    /// ended, and cancels it. Said while the call is stopped, it is not played (`stopped`).
    pub fn say(&self, text: impl Into<String>, options: SayOptions) -> Saying {
        let number = self.said.fetch_add(1, Ordering::Relaxed);
        let id = format!("{}-say-{number}", self.call_id);
        let (events, received) = unbounded();
        self.send(Message::Say {
            id: id.clone(),
            text: text.into(),
            options,
            events,
        });
        Saying::new(id, self.internal.clone(), received)
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
    models: Arc<dyn VoiceModels>,
    io: Box<dyn AudioIo>,
    loaded: Option<Models>,
    /// When the loaded models leave memory, the call stopped.
    residency: Residency,
    running: bool,
    /// Started, its microphone and speaker not yet ready.
    opening: bool,
    /// Capture not yet in a whole detector window.
    captured: VecDeque<f32>,
    /// The detector's position: samples taken into windows since its last reset.
    taken: u64,
    /// The model tasks under way (the latest of them: older ones have ended).
    tasks: VecDeque<AbortHandle>,
    /// The task's own channel, for what the microphone, the speaker and the model tasks report.
    messages: UnboundedSender<Message>,
    events: UnboundedSender<VoiceEvent>,
    /// The handles of what is being said or waits to be, by id, until each is done.
    sayers: HashMap<String, UnboundedSender<SayEvent>>,
}

impl Driver {
    async fn run(
        mut self,
        owner: UnboundedReceiver<Message>,
        reported: UnboundedReceiver<Message>,
    ) {
        // `None` once the owner's channel closed; the task's own channel never does, since the task holds a sender.
        let owner = owner.map(Some).chain(stream::once(ready(None)));
        let mut inbox = stream::select(owner, reported.map(Some));
        // What reached the task while it was busy with a message, in order.
        let mut pending = VecDeque::new();
        loop {
            let message = match pending.pop_front() {
                Some(message) => Some(Some(message)),
                None => match self.next_deadline() {
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
                },
            };
            let Some(Some(message)) = message else {
                // The VoiceCall was dropped.
                self.abandon();
                return;
            };
            match self.settle(message, &mut inbox, &mut pending).await {
                Settled::Done => {}
                // The stop waits in `pending`, and is answered next.
                Settled::Stopped => self.abandon(),
                Settled::Gone => {
                    self.abandon();
                    return;
                }
            }
            let idle = self.loaded.is_some() && !self.running;
            self.residency.observe(monotonic_ms(), idle);
        }
    }

    fn next_deadline(&self) -> Option<u64> {
        [self.call.deadline(), self.residency.deadline()]
            .into_iter()
            .flatten()
            .min()
    }

    /// Handles `message` while still hearing the owner: what arrives meanwhile waits in `pending`, but a stop, or the
    /// owner gone, still waiting after [`ABANDON_AFTER_MS`] (a model that does not answer) abandons the message.
    async fn settle(
        &mut self,
        message: Message,
        inbox: &mut (impl Stream<Item = Option<Message>> + Unpin),
        pending: &mut VecDeque<Message>,
    ) -> Settled {
        let mut work = Box::pin(self.receive(message));
        let mut abandon: Option<(u64, Settled)> = None;
        loop {
            let timer = async move {
                match abandon {
                    Some((at, _)) => sleep(at.saturating_sub(monotonic_ms())).await,
                    None => std::future::pending().await,
                }
            };
            match select(work.as_mut(), select(inbox.next(), Box::pin(timer))).await {
                Either::Left(((), _)) => {
                    return match abandon {
                        Some((_, Settled::Gone)) => Settled::Gone,
                        _ => Settled::Done,
                    }
                }
                Either::Right((Either::Left((next, _)), _)) => {
                    let at = monotonic_ms() + ABANDON_AFTER_MS;
                    match next {
                        Some(Some(Message::Stop)) => {
                            pending.push_back(Message::Stop);
                            abandon.get_or_insert((at, Settled::Stopped));
                        }
                        Some(Some(message)) => pending.push_back(message),
                        Some(None) | None => {
                            let at = abandon.map_or(at, |(earlier, _)| earlier.min(at));
                            abandon = Some((at, Settled::Gone));
                        }
                    }
                }
                Either::Right((Either::Right(((), _)), _)) => {
                    return abandon.map_or(Settled::Done, |(_, settled)| settled);
                }
            }
        }
    }

    /// Stops at once, without waiting on any model: the microphone and the speaker close, the models and every task
    /// the call started are dropped (the next start loads them again), and the call says it is idle.
    fn abandon(&mut self) {
        self.running = false;
        self.opening = false;
        self.io.stop();
        for task in self.tasks.drain(..) {
            task.abort();
        }
        self.loaded = None;
        self.captured.clear();
        self.taken = 0;
        self.input(Input::Stop);
    }

    async fn receive(&mut self, message: Message) {
        match message {
            Message::Start => self.start().await,
            Message::Stop => {
                if self.running {
                    self.halt().await;
                } else {
                    // Nothing to close: the call still says it is idle, after all it said before.
                    self.input(Input::Stop);
                }
            }
            Message::Config(config) => {
                self.configure(config);
                if self.running && self.end_of_turn_missing() {
                    self.error("end-of-turn-missing".into());
                    self.halt().await;
                }
            }
            Message::Models(models, config) => {
                self.models = models;
                self.loaded = None;
                let running = self.running;
                self.halt().await;
                self.loaded = None;
                self.configure(config);
                if running {
                    self.start().await;
                }
            }
            Message::Say {
                id,
                text,
                options,
                events,
            } => {
                self.sayers.insert(id.clone(), events);
                self.input(Input::Say {
                    id,
                    text,
                    language: options.language,
                });
            }
            Message::CancelSay(id) => self.input(Input::CancelSay(id)),
            Message::Mute(muted) => self.input(Input::Mute(muted)),
            Message::CancelInput => self.input(Input::Cancel),
            Message::Io(IoEvent::Ready) => {
                if self.running && self.opening {
                    self.opening = false;
                    self.input(Input::Start);
                }
            }
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
            Message::EndOfTurn {
                turn,
                pause,
                result,
            } => {
                self.input(Input::EndOfTurn {
                    turn,
                    pause,
                    result,
                });
            }
        }
    }

    fn configure(&mut self, config: Box<VoiceConfig>) {
        self.config = (*config).clone();
        self.residency.set_minutes(config.idle_unload_minutes);
        self.input(Input::Config(config));
    }

    /// Whether the configuration asks for `smart-turn` and the loaded models have no end-of-turn classifier.
    fn end_of_turn_missing(&self) -> bool {
        let smart = self.config.end_of_turn == EndOfTurn::SmartTurn;
        smart
            && self
                .loaded
                .as_ref()
                .is_some_and(|loaded| loaded.end_of_turn.is_none())
    }

    async fn start(&mut self) {
        if self.running {
            return;
        }
        if self.loaded.is_none() {
            match self.models.load().await {
                Ok(loaded) => self.loaded = Some(loaded),
                Err(code) => {
                    self.error(code);
                    return;
                }
            }
        }
        if self.end_of_turn_missing() {
            self.error("end-of-turn-missing".into());
            return;
        }
        let sink = IoSink(self.messages.clone());
        if let Err(code) = self.io.start(sink) {
            self.error(code);
            return;
        }
        // The call listens once the microphone and the speaker say they work (`IoEvent::Ready`).
        self.running = true;
        self.opening = true;
    }

    /// Stops the call, its microphone and its speaker, and starts the detector over.
    async fn halt(&mut self) {
        if !self.running {
            return;
        }
        self.running = false;
        self.opening = false;
        // The microphone and the speaker close before the call says it is idle; what its tasks would answer is moot.
        self.io.stop();
        for task in self.tasks.drain(..) {
            task.abort();
        }
        self.input(Input::Stop);
        self.captured.clear();
        self.taken = 0;
        if let Some(loaded) = &mut self.loaded {
            loaded.vad.reset().await;
        }
    }

    /// Runs the detector on new capture and hands each window it completed to the state machine.
    async fn captured(&mut self, pcm: &[f32]) {
        let Some(loaded) = &mut self.loaded else {
            return;
        };
        if !self.running || self.opening {
            return;
        }
        self.captured.extend(pcm);
        match loaded.vad.accept(pcm).await {
            Ok(frames) => {
                for VadFrame { end, speech, .. } in frames {
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
                loaded.vad.reset().await;
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
                Effect::Say { id, event } => {
                    let done = matches!(event, SayEvent::Done { .. });
                    if let Some(sayer) = self.sayers.get(&id) {
                        let _ = sayer.unbounded_send(event);
                    }
                    if done {
                        self.sayers.remove(&id);
                    }
                }
                Effect::Transcribe {
                    turn,
                    pcm,
                    language,
                } => {
                    let Some(loaded) = &self.loaded else { continue };
                    let transcriber = Arc::clone(&loaded.transcriber);
                    let messages = self.messages.clone();
                    self.track(async move {
                        let result = transcriber.transcribe(pcm, RATE, language).await;
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
                    let (voice, speed) = (self.config.voice.clone(), self.config.speed);
                    let language = language.or_else(|| self.config.language.clone());
                    self.track(async move {
                        let result = speaker.speak(text, voice, language, speed).await;
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
                Effect::EndOfTurn { turn, pause, pcm } => {
                    let model = self
                        .loaded
                        .as_ref()
                        .and_then(|loaded| loaded.end_of_turn.clone());
                    let Some(model) = model else { continue };
                    let messages = self.messages.clone();
                    self.track(async move {
                        let result = model.end_of_turn(pcm, RATE).await;
                        let _ = messages.unbounded_send(Message::EndOfTurn {
                            turn,
                            pause,
                            result,
                        });
                    });
                }
            }
        }
    }

    /// Runs `task` on its own, abortable: `halt` and `abandon` drop what is still running, and the models it holds.
    fn track(&mut self, task: impl Future<Output = ()> + MaybeSend + 'static) {
        let (task, handle) = abortable(task);
        self.tasks.retain(|task| !task.is_aborted());
        if self.tasks.len() >= TASKS_KEPT {
            self.tasks.pop_front();
        }
        self.tasks.push_back(handle);
        spawn(async move {
            let _ = task.await;
        });
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
