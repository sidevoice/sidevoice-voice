//! [`VoiceCall`]: one call, run as a task of its own around the state machine (`call`). The task loads the models its
//! halves use ([`VoiceSetup`]: the input half's detector, transcriber and, for `smart`, end-of-turn model; the output
//! half's speaker), each slot on its own and all at once, as the call starts, and drops them once it has been stopped
//! for its idle minutes; it opens the speaker, and the microphone only with an input half. A change of halves while it
//! runs loads what it needs beside them and takes effect at each half's safe point (`halves`), without a restart. It
//! feeds the capture to the detector and its windows to the state machine, runs each transcription, synthesis and
//! end-of-turn question as a task of its own (so a long one never holds the detector back), and does what the state
//! machine answers. A start that asks for a take has the capture kept from the moment
//! the call listens, handed over with the state machine's marks as that listening ends. The owner's calls reach it on
//! a channel only [`VoiceCall`] holds, and what the microphone, the speaker and the model tasks report on one of its
//! own; the host hears it through [`Events`]. When the owner's channel closes (the [`VoiceCall`] was dropped) the task
//! stops the call, drops the models and the microphone and speaker, and ends: what its tasks report after that goes
//! nowhere.

mod halves;
#[cfg(all(test, native))]
mod tests;

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures_channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures_channel::oneshot;
use futures_util::future::{abortable, join, ready, select, AbortHandle, Either};
use futures_util::{stream, Stream, StreamExt};

use crate::call::{Call, Effect, Input};
use crate::config::{CallConfig, EndOfTurn};
use crate::event::{VoiceError, VoiceEvent};
use crate::io::{AudioIo, IoEvent, IoSink, RenderedMailbox};
use crate::level::{instant, Level};
use crate::maybe_send::MaybeSend;
use crate::models::{Speaker, VadFrame};
use crate::residency::Residency;
use crate::runtime::{monotonic_ms, sleep, spawn, unix_ms};
use crate::say::{SayEvent, SayOptions, Saying};
use crate::setup::{VoiceInput, VoiceOutput, VoiceSetup};
use crate::take::Recording;
use crate::turns::RATE;

use halves::{load_input, InputHalf, InputSlots, OutputHalf, PendingInput, PendingOutput};

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

/// How a call starts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StartOptions {
    /// Keep the take: what the microphone gives from the moment the call listens until that listening ends, with where
    /// the person's turns fall in it, handed over then ([`VoiceEvent::Take`]). Never kept otherwise.
    pub take: bool,
}

/// What reaches the call's task.
pub(crate) enum Message {
    Start(StartOptions),
    Stop,
    /// Another setup: other halves, other models, another configuration.
    Update(Box<VoiceSetup>),
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
    /// Close the open turn now, then answer.
    EndInput(oneshot::Sender<()>),
    /// The shared native input router applies this after preceding captured frames.
    #[cfg(native)]
    SharedRoute(u64, oneshot::Sender<Result<(), String>>),
    Io(u64, IoEvent),
    Rendered(u64),
    #[cfg(native)]
    PhysicalRendered(u64, Vec<bool>, Vec<f32>),
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
    /// What loaded for the input half's change `serial`.
    InputLoaded {
        serial: u64,
        slots: InputSlots,
    },
    /// What loaded for the output half's change `serial`.
    OutputLoaded {
        serial: u64,
        result: Result<Arc<dyn Speaker>, String>,
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
    /// A call with the halves of `setup`, on the microphone and speaker of `io`. It loads and opens nothing until
    /// [`VoiceCall::start`]. Natively its task runs on the app's Tokio runtime.
    #[must_use]
    pub fn new(io: Box<dyn AudioIo>, setup: VoiceSetup) -> (Self, Events) {
        let (messages, inbox) = unbounded();
        let (internal, reported) = unbounded();
        let (events, received) = unbounded();
        let call_id = call_id();
        let config = setup.call_config();
        let driver = Driver {
            call: Call::new(
                config.clone(),
                call_id.clone(),
                unix_ms().saturating_sub(monotonic_ms()),
            ),
            residency: Residency::new(config.call.idle_unload_minutes),
            config,
            input: setup.input.map(|input| InputHalf::new(input.models)),
            output: setup.output.map(|output| OutputHalf::new(output.models)),
            pending_input: None,
            pending_output: None,
            changes: 0,
            unopened: None,
            io,
            running: false,
            opening: false,
            tasks: VecDeque::new(),
            captured: VecDeque::new(),
            rendered: VecDeque::new(),
            playback_level: Level::default(),
            rendered_mailbox: Arc::new(Mutex::new(RenderedMailbox::default())),
            generation: 0,
            taken: 0,
            taking: false,
            recording: None,
            muted: false,
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

    /// Loads the models its halves use if they are not, opens the speaker and, with an input half, the microphone, and
    /// starts listening (without an input half, it only speaks). With
    /// `options.take`, it keeps what the microphone gives from the moment it listens until that listening ends, and
    /// hands it over then as [`VoiceEvent::Take`] (a restart on other models ends one stretch and starts the next).
    /// Started already, it changes nothing.
    pub fn start(&self, options: StartOptions) {
        self.send(Message::Start(options));
    }

    /// Stops listening and speaking: the person's turn not yet reported is cancelled, and what is being said or
    /// waits to be is not (`stopped`). The models stay loaded for the next start.
    pub fn stop(&self) {
        self.send(Message::Stop);
    }

    /// Another setup, without a restart. A half whose models are the same (the same `Arc`) keeps what it loaded; one
    /// with other models loads them anew. Stopped, it is in effect at once (the next start loads what is missing).
    /// Running, the call's own configuration is in effect at once, and each half's change loads what it needs while
    /// the half goes on as it was, then takes effect at its safe point: the input half's at the next turn (no turn
    /// open), the output half's at the next utterance (what is being said ends as it began; what is asked meanwhile
    /// waits for the change). Nothing playing is cut. An input half that comes opens the microphone
    /// ([`AudioIo::set_input`]); one that goes closes it. A model that cannot load (`smart` without an end-of-turn
    /// model, `end-of-turn-missing`) is a [`VoiceEvent::Error`] with its code, and the half stays as it was.
    ///
    /// A microphone lost under a running call ([`IoEvent::InputLost`]) ends the open turn with what it holds, says the
    /// code as an error and leaves the call with its output half alone (`listening: off`); an `update` with an input
    /// half adds it back. Without an output half there is nothing left, and the call stops.
    pub fn update(&self, setup: VoiceSetup) {
        self.send(Message::Update(Box::new(setup)));
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

    /// Closes the person's open turn now with what was captured so far: it is transcribed and reported like any turn
    /// (`finished`, or `cancelled` when nothing was said). Nothing is open, or the call is stopped: nothing happens.
    /// It touches neither the playback, nor the mute, nor how later turns end. The message is sent when this is called;
    /// the future resolves once the call has closed the turn (or at once if the call has ended).
    pub fn end_input(&self) -> impl Future<Output = ()> + Send + 'static {
        let (done, closed) = oneshot::channel();
        self.send(Message::EndInput(done));
        async move {
            let _ = closed.await;
        }
    }

    fn send(&self, message: Message) {
        let _ = self.messages.unbounded_send(message);
    }
}

/// The call's task.
struct Driver {
    call: Call,
    config: CallConfig,
    input: Option<InputHalf>,
    output: Option<OutputHalf>,
    /// A change of each half waiting for its models and its safe point, while the call runs.
    pending_input: Option<PendingInput>,
    pending_output: Option<PendingOutput>,
    /// How many changes of a half there were: each pending change's serial.
    changes: u64,
    /// Why the microphone did not open while the call was opening without it: said once the call works.
    unopened: Option<String>,
    io: Box<dyn AudioIo>,
    /// When the loaded models leave memory, the call stopped.
    residency: Residency,
    running: bool,
    /// Started, its microphone and speaker not yet ready.
    opening: bool,
    /// Capture not yet in a whole detector window.
    captured: VecDeque<f32>,
    /// Rendered 16 kHz speaker samples waiting for capture's detector windows.
    rendered: VecDeque<f32>,
    playback_level: Level,
    rendered_mailbox: Arc<Mutex<RenderedMailbox>>,
    /// A new start rejects any queued report from an older microphone or speaker.
    generation: u64,
    /// The detector's position: samples taken into windows since its last reset.
    taken: u64,
    /// Whether the start asked for a take, until the call is stopped.
    taking: bool,
    /// The take being kept: from the moment the call listened, while it does.
    recording: Option<Recording>,
    /// Whether the person muted the microphone: a take keeps silence meanwhile.
    muted: bool,
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
                                self.apply_pending(false);
                                if self.residency.due(now) {
                                    // Dropping them is what unloads them; the next start loads them again.
                                    self.unload();
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
            if self.running {
                self.apply_pending(false);
            }
            let idle = self.loaded() && !self.running;
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
        self.end_take();
        self.reset_playback();
        self.generation = self.generation.wrapping_add(1);
        self.running = false;
        self.opening = false;
        self.unopened = None;
        self.io.stop();
        for task in self.tasks.drain(..) {
            task.abort();
        }
        self.apply_pending(true);
        self.unload();
        self.captured.clear();
        self.taken = 0;
        self.input(Input::Stop);
    }

    async fn receive(&mut self, message: Message) {
        match message {
            Message::Start(options) => {
                if !self.running {
                    self.taking = options.take;
                }
                self.start().await;
            }
            Message::Stop => {
                if self.running {
                    self.halt().await;
                } else {
                    // Nothing to close: the call still says it is idle, after all it said before.
                    self.input(Input::Stop);
                }
                self.taking = false;
            }
            Message::Update(setup) => self.update(*setup).await,
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
            Message::Mute(muted) => {
                self.muted = muted;
                self.input(Input::Mute(muted));
            }
            Message::CancelInput => self.input(Input::Cancel),
            Message::EndInput(done) => {
                self.input(Input::EndTurn);
                let _ = done.send(());
            }
            #[cfg(native)]
            Message::SharedRoute(_, _) => {}
            #[cfg(native)]
            Message::PhysicalRendered(_, _, _) => {}
            Message::Io(generation, _) if generation != self.generation => {}
            Message::Rendered(generation) => {
                let frame = self
                    .rendered_mailbox
                    .lock()
                    .expect("rendered mailbox")
                    .take(generation);
                if generation == self.generation && self.running && !self.opening {
                    if let Some(frame) = frame {
                        let stale = monotonic_ms().saturating_sub(frame.at_ms) > 500;
                        if frame.skipped || stale {
                            self.rendered.clear();
                            self.playback_level.reset();
                        }
                        if !stale {
                            self.accept_rendered(&frame.samples);
                        }
                    }
                }
            }
            Message::Io(_, IoEvent::Ready) => {
                if self.running && self.opening {
                    self.opening = false;
                    // Sample 0 of a take, when one is asked for.
                    self.recording = self.taking.then(|| Recording::new(unix_ms()));
                    self.input(Input::Start { take: self.taking });
                    // A microphone that could not open is said once the call works without it.
                    if let Some(code) = self.unopened.take() {
                        self.error(code);
                    }
                }
            }
            Message::Io(_, IoEvent::Captured(pcm)) => self.captured(&pcm).await,
            Message::Io(_, IoEvent::Rendered(pcm)) => {
                self.accept_rendered(&pcm);
            }
            Message::Io(_, IoEvent::PlaybackEnded) => {
                self.input(Input::PlaybackEnded);
                self.reset_playback();
            }
            Message::Io(_, IoEvent::ChunkStarted { utterance, chunk }) => {
                self.input(Input::ChunkStarted { utterance, chunk });
            }
            Message::Io(_, IoEvent::ChunkPlayed { utterance, chunk }) => {
                self.input(Input::ChunkPlayed { utterance, chunk });
            }
            Message::Io(_, IoEvent::MicrophoneMuted(muted)) => {
                self.input(Input::MicrophoneMuted(muted));
            }
            Message::Io(_, IoEvent::InputLost(code)) => {
                if self.running {
                    self.input_lost(code).await;
                }
            }
            Message::InputLoaded { serial, mut slots } => {
                if let Some(pending) = self
                    .pending_input
                    .as_mut()
                    .filter(|pending| pending.serial == serial)
                {
                    match slots.failure.take() {
                        Some(code) => {
                            self.pending_input = None;
                            self.error(code);
                        }
                        None => {
                            pending.slots = slots;
                            pending.ready = true;
                        }
                    }
                }
            }
            Message::OutputLoaded { serial, result } => {
                if let Some(pending) = self
                    .pending_output
                    .as_mut()
                    .filter(|pending| pending.serial == serial)
                {
                    match result {
                        Ok(speaker) => {
                            pending.speaker = Some(speaker);
                            pending.ready = true;
                        }
                        Err(code) => {
                            self.pending_output = None;
                            if self.output.is_none() {
                                // The half that was to join does not: what waited for it is not played.
                                let mut config = self.config.clone();
                                config.output = None;
                                self.configure(config);
                            }
                            self.input(Input::HoldOutput(false));
                            self.error(code);
                        }
                    }
                }
            }
            Message::Io(_, IoEvent::Failed(code)) => {
                self.end_take();
                self.reset_playback();
                self.error(code);
                self.halt().await;
            }
            Message::Transcribed { turn, result } => {
                self.input(Input::Transcribed { turn, result })
            }
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

    fn configure(&mut self, config: CallConfig) {
        self.residency.set_minutes(config.call.idle_unload_minutes);
        self.config = config.clone();
        self.input(Input::Config(Box::new(config)));
    }

    /// Takes another setup. A half with the same models keeps what it loaded. Stopped, every change is in effect at
    /// once (what the next start needs, it loads). Running, the call's own configuration is in effect at once, and each
    /// half's change waits for what it needs to load (loaded meanwhile, the half working on as it was) and for its safe
    /// point: the input's at the next turn, the output's at the next utterance, never cutting what plays.
    async fn update(&mut self, setup: VoiceSetup) {
        let VoiceSetup {
            input,
            output,
            config,
        } = setup;
        if !self.running {
            self.pending_input = None;
            self.pending_output = None;
            let mut next = self.config.clone();
            next.call = config;
            next.input = input.as_ref().map(|input| input.config.clone());
            next.output = output.as_ref().map(|output| output.config.clone());
            self.input = input.map(|input| match self.input.take() {
                Some(half) if Arc::ptr_eq(&half.models, &input.models) => half,
                _ => InputHalf::new(input.models),
            });
            self.output = output.map(|output| match self.output.take() {
                Some(half) if Arc::ptr_eq(&half.models, &output.models) => half,
                _ => OutputHalf::new(output.models),
            });
            self.configure(next);
            return;
        }
        let mut next = self.config.clone();
        next.call = config;
        self.configure(next);
        self.change_input(input);
        self.change_output(output);
        self.apply_pending(false);
    }

    /// The input half is to become `target` (or go, with `None`): what it needs starts loading now.
    fn change_input(&mut self, target: Option<VoiceInput>) {
        let same_models = match (&self.input, &target) {
            (None, None) => true,
            (Some(half), Some(input)) => Arc::ptr_eq(&half.models, &input.models),
            _ => false,
        };
        if same_models && target.as_ref().map(|input| &input.config) == self.config.input.as_ref() {
            self.pending_input = None;
            return;
        }
        self.changes += 1;
        let serial = self.changes;
        let target = target.map(|input| (input.models, input.config));
        let (whole, end_of_turn) = match &target {
            None => (false, false),
            Some((models, config)) => {
                let smart = config.end_of_turn == EndOfTurn::Smart {};
                match &self.input {
                    Some(half) if Arc::ptr_eq(&half.models, models) => half.missing(smart),
                    _ => (true, smart),
                }
            }
        };
        if let (Some((models, _)), true) = (&target, whole || end_of_turn) {
            let (models, messages) = (Arc::clone(models), self.messages.clone());
            self.track(async move {
                let slots = load_input(models, whole, end_of_turn).await;
                let _ = messages.unbounded_send(Message::InputLoaded { serial, slots });
            });
        }
        self.pending_input = Some(PendingInput {
            target,
            slots: InputSlots::default(),
            ready: !whole && !end_of_turn,
            serial,
        });
    }

    /// The output half is to become `target` (or go): its speaker starts loading now, and nothing new starts to be
    /// said until the change is in effect.
    fn change_output(&mut self, target: Option<VoiceOutput>) {
        let same_models = match (&self.output, &target) {
            (None, None) => true,
            (Some(half), Some(output)) => Arc::ptr_eq(&half.models, &output.models),
            _ => false,
        };
        if same_models
            && target.as_ref().map(|output| &output.config) == self.config.output.as_ref()
        {
            if self.pending_output.take().is_some() {
                self.input(Input::HoldOutput(false));
            }
            return;
        }
        self.changes += 1;
        let serial = self.changes;
        let target = target.map(|output| (output.models, output.config));
        let load = !same_models && target.is_some();
        if let (Some((models, _)), true) = (&target, load) {
            let (models, messages) = (Arc::clone(models), self.messages.clone());
            self.track(async move {
                let result = models.speaker().await;
                let _ = messages.unbounded_send(Message::OutputLoaded { serial, result });
            });
        }
        self.input(Input::HoldOutput(true));
        // An output half that joins takes what is asked from now on, to be said once its speaker is in.
        if let (None, Some((_, output))) = (&self.config.output, &target) {
            let mut config = self.config.clone();
            config.output = Some(output.clone());
            self.configure(config);
        }
        self.pending_output = Some(PendingOutput {
            target,
            speaker: None,
            ready: !load,
            serial,
        });
    }

    /// Puts each pending change in effect once it is ready and at its safe point; with `now`, at once, ready or not,
    /// with what loaded for it (the call has stopped: there is nothing to wait for).
    fn apply_pending(&mut self, now: bool) {
        let input_due = self
            .pending_input
            .as_ref()
            .is_some_and(|pending| now || (pending.ready && self.call.input_idle()));
        if input_due {
            let pending = self.pending_input.take().expect("a pending input change");
            let had = self.input.is_some();
            let mut config = self.config.clone();
            let mut fresh = false;
            match pending.target {
                None => {
                    self.input = None;
                    config.input = None;
                }
                Some((models, input)) => {
                    let smart = input.end_of_turn == EndOfTurn::Smart {};
                    let mut half = match self.input.take() {
                        Some(half) if Arc::ptr_eq(&half.models, &models) => half,
                        _ => InputHalf::new(models),
                    };
                    fresh = pending.slots.whole();
                    half.take(pending.slots, smart);
                    self.input = Some(half);
                    config.input = Some(input);
                }
            }
            let has = self.input.is_some();
            if fresh || had != has {
                // Another detector stream, or none: its positions start over.
                self.captured.clear();
                self.taken = 0;
            }
            if had != has && self.running {
                self.io.set_input(has);
            }
            self.configure(config);
        }
        let output_due = self
            .pending_output
            .as_ref()
            .is_some_and(|pending| now || (pending.ready && self.call.output_idle()));
        if output_due {
            let pending = self.pending_output.take().expect("a pending output change");
            let mut config = self.config.clone();
            match pending.target {
                None => {
                    self.output = None;
                    config.output = None;
                }
                Some((models, output)) => {
                    let mut half = match self.output.take() {
                        Some(half) if Arc::ptr_eq(&half.models, &models) => half,
                        _ => OutputHalf::new(models),
                    };
                    if pending.speaker.is_some() {
                        half.speaker = pending.speaker;
                    }
                    self.output = Some(half);
                    config.output = Some(output);
                }
            }
            self.configure(config);
            self.input(Input::HoldOutput(false));
        }
    }

    /// The microphone went, or could not open. With an output half the call goes on speaking: the open turn is told
    /// with what it holds, the input half goes, and the failure is said. Without one there is nothing left: it stops.
    async fn input_lost(&mut self, code: String) {
        if self.input.is_none() {
            return;
        }
        if self.output.is_none() {
            self.end_take();
            self.reset_playback();
            self.error(code);
            self.halt().await;
            return;
        }
        self.end_take();
        // The turn under way is transcribed by the task this starts, which holds its own transcriber.
        self.input(Input::EndTurn);
        if self.opening {
            self.unopened = Some(code);
        } else {
            self.error(code);
        }
        self.pending_input = None;
        self.input = None;
        self.captured.clear();
        self.taken = 0;
        let mut config = self.config.clone();
        config.input = None;
        self.configure(config);
    }

    /// Whether any model is loaded.
    fn loaded(&self) -> bool {
        self.input
            .as_ref()
            .is_some_and(|half| half.loaded.is_some())
            || self
                .output
                .as_ref()
                .is_some_and(|half| half.speaker.is_some())
    }

    /// Drops every model loaded; dropping them is what unloads them.
    fn unload(&mut self) {
        if let Some(half) = &mut self.input {
            half.loaded = None;
        }
        if let Some(half) = &mut self.output {
            half.speaker = None;
        }
    }

    /// Loads what the halves use and is not loaded yet, every slot at once: the detector and the transcriber, the
    /// end-of-turn model only for `smart` (dropped otherwise), the speaker. What loaded stays loaded when another slot
    /// fails; the first failure's code is the answer.
    async fn load(&mut self) -> Result<(), String> {
        let smart = smart(&self.config);
        let input = self.input.as_ref().map(|half| {
            let (whole, end_of_turn) = half.missing(smart);
            load_input(Arc::clone(&half.models), whole, end_of_turn)
        });
        let output = self
            .output
            .as_ref()
            .filter(|half| half.speaker.is_none())
            .map(|half| {
                let models = Arc::clone(&half.models);
                async move { models.speaker().await }
            });
        let input = async move {
            match input {
                Some(input) => Some(input.await),
                None => None,
            }
        };
        let output = async move {
            match output {
                Some(output) => Some(output.await),
                None => None,
            }
        };
        let (input, output) = join(input, output).await;
        let mut failure = None;
        if let (Some(half), Some(speaker)) = (&mut self.output, output) {
            match speaker {
                Ok(speaker) => half.speaker = Some(speaker),
                Err(code) => failure = Some(code),
            }
        }
        if let (Some(half), Some(mut slots)) = (&mut self.input, input) {
            failure = failure.or(slots.failure.take());
            half.take(slots, smart);
        }
        failure.map_or(Ok(()), Err)
    }

    async fn start(&mut self) {
        if self.running {
            return;
        }
        if let Err(code) = self.load().await {
            self.error(code);
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        self.reset_playback();
        self.rendered_mailbox
            .lock()
            .expect("rendered mailbox")
            .advance(self.generation);
        let sink = IoSink {
            messages: self.messages.clone(),
            generation: self.generation,
            rendered: Arc::clone(&self.rendered_mailbox),
            direct_rendered: false,
        };
        if let Err(code) = self.io.start(sink, self.input.is_some()) {
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
        self.end_take();
        self.reset_playback();
        self.generation = self.generation.wrapping_add(1);
        self.running = false;
        self.opening = false;
        self.unopened = None;
        // The microphone and the speaker close before the call says it is idle; what its tasks would answer is moot.
        self.io.stop();
        for task in self.tasks.drain(..) {
            task.abort();
        }
        self.input(Input::Stop);
        self.apply_pending(true);
        self.captured.clear();
        self.taken = 0;
        if let Some(loaded) = self.input.as_mut().and_then(|half| half.loaded.as_mut()) {
            loaded.vad.reset().await;
        }
    }

    /// Runs the detector on new capture and hands each window it completed to the state machine.
    async fn captured(&mut self, pcm: &[f32]) {
        let Some(loaded) = self.input.as_mut().and_then(|half| half.loaded.as_mut()) else {
            return;
        };
        if !self.running || self.opening {
            return;
        }
        self.captured.extend(pcm);
        if let Some(recording) = &mut self.recording {
            recording.extend(pcm, self.muted);
        }
        match loaded.vad.accept(pcm).await {
            Ok(frames) => {
                for VadFrame {
                    end,
                    speech,
                    probability,
                } in frames
                {
                    let take = (end.saturating_sub(self.taken) as usize).min(self.captured.len());
                    self.taken = end;
                    let window: Vec<f32> = self.captured.drain(..take).collect();
                    self.playback_window(take);
                    self.input(Input::Window {
                        pcm: window,
                        speech,
                        probability,
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

    /// One speaker meter publication per detector window, using the same sample count as the microphone window.
    fn playback_window(&mut self, length: usize) {
        let mut window = Vec::with_capacity(length);
        for _ in 0..length {
            window.push(self.rendered.pop_front().unwrap_or(0.0));
        }
        if instant(&window) == 0.0 {
            self.playback_level.reset();
        } else {
            self.playback_level.take(&window);
        }
        let _ = self
            .events
            .unbounded_send(VoiceEvent::PlaybackLevel(self.playback_level.value()));
    }

    fn reset_playback(&mut self) {
        self.rendered.clear();
        self.playback_level.reset();
        let _ = self.events.unbounded_send(VoiceEvent::PlaybackLevel(0.0));
    }

    fn accept_rendered(&mut self, pcm: &[f32]) {
        if self.running && !self.opening {
            // A silent start must not queue seconds of zeros ahead of the first audible sample.
            if !self.rendered.is_empty() || pcm.iter().any(|sample| *sample != 0.0) {
                self.rendered.extend(pcm);
                while self.rendered.len() > 4_096 {
                    self.rendered.pop_front();
                }
            }
        }
    }

    /// Hands over the take being kept, if one is, with the marks the call kept for it: the stretch of listening it
    /// covers has ended.
    fn end_take(&mut self) {
        if let Some(recording) = self.recording.take() {
            let take = recording.finish(self.call.take_marks());
            let _ = self.events.unbounded_send(VoiceEvent::Take(take));
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
                    let loaded = self.input.as_ref().and_then(|half| half.loaded.as_ref());
                    let Some(loaded) = loaded else { continue };
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
                    let speaker = self.output.as_ref().and_then(|half| half.speaker.clone());
                    let Some(speaker) = speaker else { continue };
                    let Some(output) = &self.config.output else {
                        continue;
                    };
                    let messages = self.messages.clone();
                    let (voice, speed) = (output.voice.clone(), output.speed);
                    let language = language.or_else(|| self.config.say_language());
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
                        .input
                        .as_ref()
                        .and_then(|half| half.loaded.as_ref())
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

/// Whether the configuration's input half ends turns with the end-of-turn model.
fn smart(config: &CallConfig) -> bool {
    config
        .input
        .as_ref()
        .is_some_and(|input| input.end_of_turn == EndOfTurn::Smart {})
}

/// An id for a call, unique on this device: the time it was made and a counter, in hexadecimal.
fn call_id() -> String {
    static CALLS: AtomicU64 = AtomicU64::new(0);
    format!("{:x}{:x}", unix_ms(), CALLS.fetch_add(1, Ordering::Relaxed))
}
