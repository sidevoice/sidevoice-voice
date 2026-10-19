//! The call's task on fakes: models that detect on energy, transcribe to a fixed text and speak silence, and a
//! microphone and speaker the test plays by hand, on a Tokio runtime as an app runs it.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;

use super::{Events, StartOptions, VoiceCall};
use crate::config::{CallConfig, Patience};
use crate::io::{AudioIo, IoEvent, IoSink};
use crate::models::{
    EndOfTurnModel, InputModels, OutputModels, SharedModelLoader, SharedModels, Speaker,
    Transcriber, Vad, VadFactory, VadFrame,
};
use crate::say::{SayEvent, SayOptions, SayOutcome};
use crate::setup::{VoiceInput, VoiceOutput, VoiceSetup};
use crate::test_support::{clip, config, config_with, silence, EnergyVad, QUILTER, WINDOW};
use crate::TurnEvent;
use crate::VoiceEvent;

#[derive(Default)]
struct FakeModels {
    fail: Option<&'static str>,
    /// The end-of-turn classifier it supplies, if any.
    end_of_turn: Option<Arc<dyn EndOfTurnModel>>,
    /// How many times the detector was loaded, and how many detectors are alive (one per load not dropped yet).
    loads: Arc<AtomicUsize>,
    alive: Arc<AtomicUsize>,
    /// How many times the transcriber, the speaker and the end-of-turn model were loaded.
    transcribers: Arc<AtomicUsize>,
    speakers: Arc<AtomicUsize>,
    classifiers: Arc<AtomicUsize>,
    /// The voice each chunk was spoken with.
    voices: Arc<Mutex<Vec<Option<String>>>>,
    /// Once set, the detector never answers again.
    hang: Arc<AtomicBool>,
}

#[async_trait]
impl InputModels for FakeModels {
    async fn vad(&self) -> Result<Box<dyn Vad>, String> {
        if let Some(code) = self.fail {
            return Err(code.into());
        }
        self.loads.fetch_add(1, Ordering::SeqCst);
        self.alive.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeDetector {
            vad: EnergyVad::new(),
            pending: Vec::new(),
            end: 0,
            alive: Arc::clone(&self.alive),
            hang: Arc::clone(&self.hang),
        }))
    }

    async fn transcriber(&self) -> Result<Arc<dyn Transcriber>, String> {
        self.transcribers.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(FakeTranscriber))
    }

    async fn end_of_turn(&self) -> Result<Arc<dyn EndOfTurnModel>, String> {
        self.classifiers.fetch_add(1, Ordering::SeqCst);
        self.end_of_turn
            .clone()
            .ok_or_else(|| "end-of-turn-missing".into())
    }
}

#[async_trait]
impl OutputModels for FakeModels {
    async fn speaker(&self) -> Result<Arc<dyn Speaker>, String> {
        self.speakers.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(FakeSpeaker(Arc::clone(&self.voices))))
    }
}

struct FakeDetector {
    vad: EnergyVad,
    pending: Vec<f32>,
    end: u64,
    alive: Arc<AtomicUsize>,
    hang: Arc<AtomicBool>,
}

impl Drop for FakeDetector {
    fn drop(&mut self) {
        self.alive.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl Vad for FakeDetector {
    async fn accept(&mut self, pcm: &[f32]) -> Result<Vec<VadFrame>, String> {
        if self.hang.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        self.pending.extend_from_slice(pcm);
        let whole = self.pending.len() / WINDOW * WINDOW;
        let windows: Vec<f32> = self.pending.drain(..whole).collect();
        Ok(windows
            .chunks(WINDOW)
            .map(|window| {
                self.end += WINDOW as u64;
                VadFrame {
                    end: self.end,
                    speech: self.vad.window(window),
                    probability: None,
                }
            })
            .collect())
    }

    async fn reset(&mut self) {
        self.pending.clear();
        self.end = 0;
        self.vad = EnergyVad::new();
    }
}

struct FakeTranscriber;

#[async_trait]
impl Transcriber for FakeTranscriber {
    async fn transcribe(
        &self,
        pcm: Vec<f32>,
        sample_rate: u32,
        language: Option<String>,
    ) -> Result<String, String> {
        assert!(pcm.len() > 16_000);
        assert_eq!(sample_rate, 16_000);
        assert_eq!(language.as_deref(), Some("en"));
        Ok("Mister Quilter is the apostle of the middle classes.".into())
    }
}

/// Speaks silence, and records the voice of each chunk.
#[derive(Default)]
struct FakeSpeaker(Arc<Mutex<Vec<Option<String>>>>);

#[async_trait]
impl Speaker for FakeSpeaker {
    async fn speak(
        &self,
        text: String,
        voice: Option<String>,
        _language: Option<String>,
        _speed: f32,
    ) -> Result<(Vec<f32>, u32), String> {
        self.0.lock().unwrap().push(voice);
        Ok((vec![0.0; text.len() * 10], 24_000))
    }
}

/// What the call did to the speaker, and the sink it was given.
#[derive(Default)]
struct Speakers {
    /// Whether `start` leaves the microphone opening: the test says when it is ready.
    opening: bool,
    sink: Option<IoSink>,
    /// Whether the microphone was asked for.
    input: bool,
    /// Whether what is played waits for the test to say when each chunk starts and ends.
    held: bool,
    played: Vec<(String, usize, usize, u32)>,
    stopped: usize,
}

struct FakeIo(Arc<Mutex<Speakers>>);

impl AudioIo for FakeIo {
    fn start(&mut self, sink: IoSink, input: bool) -> Result<(), String> {
        let mut speakers = self.0.lock().unwrap();
        speakers.input = input;
        if !speakers.opening {
            sink.send(IoEvent::Ready);
        }
        speakers.sink = Some(sink);
        Ok(())
    }

    fn play(&mut self, utterance: &str, chunk: usize, samples: Vec<f32>, sample_rate: u32) {
        let mut speakers = self.0.lock().unwrap();
        speakers
            .played
            .push((utterance.into(), chunk, samples.len(), sample_rate));
        if speakers.held {
            return;
        }
        let sink = speakers.sink.clone().unwrap();
        sink.send(IoEvent::ChunkStarted {
            utterance: utterance.into(),
            chunk,
        });
        sink.send(IoEvent::ChunkPlayed {
            utterance: utterance.into(),
            chunk,
        });
    }

    fn set_input(&mut self, input: bool) {
        self.0.lock().unwrap().input = input;
    }

    fn stop_playback(&mut self) {
        self.0.lock().unwrap().stopped += 1;
    }

    fn stop(&mut self) {
        self.0.lock().unwrap().sink = None;
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_time()
        .build()
        .unwrap()
}

#[test]
fn rendered_output_uses_detector_windows_even_with_a_muted_microphone_and_rejects_old_sessions() {
    runtime().block_on(async {
        let (call, mut events, speakers) = call(FakeModels::default());
        call.start(StartOptions::default());
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Listening => Some(()),
            _ => None,
        })
        .await;
        let old = speakers.lock().unwrap().sink.clone().unwrap();
        call.mute(true);
        old.send(IoEvent::Rendered(vec![0.0; 16_000]));
        old.send(IoEvent::Rendered(vec![0.5; 512 * 3]));
        old.send(IoEvent::Captured(vec![0.0; 512 * 3]));
        let mut levels = Vec::new();
        for _ in 0..3 {
            levels.push(
                next(&mut events, |event| match event {
                    VoiceEvent::PlaybackLevel(level) if level > 0.0 => Some(level),
                    _ => None,
                })
                .await,
            );
        }
        assert!(levels[0] < levels[1] && levels[1] < levels[2]);
        old.send(IoEvent::PlaybackEnded);
        let zero = next(&mut events, |event| match event {
            VoiceEvent::PlaybackLevel(level) if level == 0.0 => Some(level),
            _ => None,
        })
        .await;
        assert_eq!(zero, 0.0);
        call.stop();
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Idle => Some(()),
            _ => None,
        })
        .await;
        call.start(StartOptions::default());
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Muted => Some(()),
            _ => None,
        })
        .await;
        let current = speakers.lock().unwrap().sink.clone().unwrap();
        old.send(IoEvent::Rendered(vec![1.0; 512]));
        current.send(IoEvent::Captured(vec![0.0; 512]));
        let level = next(&mut events, |event| match event {
            VoiceEvent::PlaybackLevel(level) => Some(level),
            _ => None,
        })
        .await;
        assert_eq!(level, 0.0);
    });
}

/// Both halves, fast patience, no grace.
fn fast() -> CallConfig {
    let mut config = config_with(|input| input.patience = Patience::Fast);
    config.call.audio_grace_ms = 0;
    config
}

/// `config`'s halves on `models`.
fn setup<M: InputModels + OutputModels + 'static>(
    models: &Arc<M>,
    config: CallConfig,
) -> VoiceSetup {
    VoiceSetup {
        input: config.input.map(|config| VoiceInput {
            models: Arc::clone(models) as Arc<dyn InputModels>,
            config,
        }),
        output: config.output.map(|config| VoiceOutput {
            models: Arc::clone(models) as Arc<dyn OutputModels>,
            config,
        }),
        config: config.call,
    }
}

fn call(models: FakeModels) -> (VoiceCall, Events, Arc<Mutex<Speakers>>) {
    with_config(models, fast())
}

fn with_config(
    models: FakeModels,
    config: CallConfig,
) -> (VoiceCall, Events, Arc<Mutex<Speakers>>) {
    with_setup(setup(&Arc::new(models), config))
}

fn with_setup(setup: VoiceSetup) -> (VoiceCall, Events, Arc<Mutex<Speakers>>) {
    let speakers = Arc::default();
    let (call, events) = VoiceCall::new(Box::new(FakeIo(Arc::clone(&speakers))), setup);
    (call, events, speakers)
}

/// The next event that `want` keeps, within five seconds.
async fn next<T>(events: &mut Events, want: impl Fn(VoiceEvent) -> Option<T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.next().await.expect("the call is alive");
            if let Some(found) = want(event) {
                return found;
            }
        }
    })
    .await
    .expect("the event came in time")
}

#[test]
fn speech_becomes_a_turn_and_a_reply_is_played_and_heard() {
    runtime().block_on(async {
        let (call, mut events, speakers) = call(FakeModels::default());
        call.start(StartOptions::default());
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Listening => Some(()),
            _ => None,
        })
        .await;
        let sink = speakers.lock().unwrap().sink.clone().expect("started");
        for frame in [silence(300), clip(QUILTER, 0.9), silence(2_600)]
            .concat()
            .chunks(160)
        {
            sink.send(IoEvent::Captured(frame.to_vec()));
        }
        let said = next(&mut events, |event| match event {
            VoiceEvent::Turn(TurnEvent::Finished { text, .. }) => Some(text),
            _ => None,
        })
        .await;
        assert_eq!(said, "Mister Quilter is the apostle of the middle classes.");

        let mut saying = call.say(
            "Noted. I will read the apostle's gospel tonight.",
            SayOptions::default(),
        );
        let mut steps = Vec::new();
        while let Some(step) = tokio::time::timeout(Duration::from_secs(5), saying.next())
            .await
            .expect("in time")
        {
            steps.push(step);
        }
        assert_eq!(steps.first(), Some(&SayEvent::Playing));
        assert_eq!(
            steps.last(),
            Some(&SayEvent::Done {
                outcome: SayOutcome::Heard
            })
        );
        assert!(steps.contains(&SayEvent::Progress {
            sounding: None,
            heard_chars: 48
        }));
        let played = speakers.lock().unwrap().played.clone();
        assert_eq!(played.len(), 1);
        assert_eq!((played[0].1, played[0].3), (0, 24_000));

        call.stop();
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Idle => Some(()),
            _ => None,
        })
        .await;
        assert!(speakers.lock().unwrap().sink.is_none());
    });
}

#[test]
fn a_model_that_cannot_load_is_an_error_and_the_call_stays_idle() {
    runtime().block_on(async {
        let (call, mut events, speakers) = call(FakeModels {
            fail: Some("model-not-found"),
            ..FakeModels::default()
        });
        call.start(StartOptions::default());
        let code = next(&mut events, |event| match event {
            VoiceEvent::Error(error) => Some(error.code),
            _ => None,
        })
        .await;
        assert_eq!(code, "model-not-found");
        assert!(speakers.lock().unwrap().sink.is_none());
    });
}

#[test]
fn smart_turn_without_an_end_of_turn_model_is_refused() {
    runtime().block_on(async {
        let models = Arc::new(FakeModels::default());
        let (call, mut events, _speakers) = with_setup(setup(&models, fast()));
        call.update(setup(
            &models,
            config_with(|input| input.end_of_turn = crate::EndOfTurn::Smart {}),
        ));
        call.start(StartOptions::default());
        let code = next(&mut events, |event| match event {
            VoiceEvent::Error(error) => Some(error.code),
            _ => None,
        })
        .await;
        assert_eq!(code, "end-of-turn-missing");
    });
}

/// Waits, a little at a time, until `done` holds, for at most five seconds.
async fn until(done: impl Fn() -> bool) {
    for _ in 0..500 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("not in time");
}

#[test]
fn idle_models_leave_memory_and_come_back_on_the_next_start() {
    runtime().block_on(async {
        let models = FakeModels::default();
        let (loads, alive) = (Arc::clone(&models.loads), Arc::clone(&models.alive));
        let listening = |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Listening => Some(()),
            _ => None,
        };
        // Zero minutes: they leave as soon as the call stops.
        let mut zero = config();
        zero.call.idle_unload_minutes = 0;
        let (call, mut events, _speakers) = with_config(models, zero);
        call.start(StartOptions::default());
        next(&mut events, listening).await;
        assert_eq!(
            (loads.load(Ordering::SeqCst), alive.load(Ordering::SeqCst)),
            (1, 1)
        );
        call.stop();
        until(|| alive.load(Ordering::SeqCst) == 0).await;
        call.start(StartOptions::default());
        next(&mut events, listening).await;
        assert_eq!(
            (loads.load(Ordering::SeqCst), alive.load(Ordering::SeqCst)),
            (2, 1),
            "loaded again"
        );
    });
}

struct LeaseLoader {
    loads: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
}

struct LeaseText(Arc<AtomicUsize>);

impl Drop for LeaseText {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl Transcriber for LeaseText {
    async fn transcribe(&self, _: Vec<f32>, _: u32, _: Option<String>) -> Result<String, String> {
        Ok(String::new())
    }
}

struct LeaseVadFactory;

#[async_trait]
impl VadFactory for LeaseVadFactory {
    async fn new_stream(&self) -> Result<Box<dyn Vad>, String> {
        Ok(Box::new(FakeDetector {
            vad: EnergyVad::new(),
            pending: Vec::new(),
            end: 0,
            alive: Arc::new(AtomicUsize::new(1)),
            hang: Arc::new(AtomicBool::new(false)),
        }))
    }
}

#[async_trait]
impl SharedModelLoader for LeaseLoader {
    async fn vad(&self) -> Result<Arc<dyn VadFactory>, String> {
        Ok(Arc::new(LeaseVadFactory))
    }

    async fn transcriber(&self) -> Result<Arc<dyn Transcriber>, String> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(LeaseText(Arc::clone(&self.drops))))
    }

    async fn speaker(&self) -> Result<Arc<dyn Speaker>, String> {
        Ok(Arc::new(FakeSpeaker::default()))
    }
}

#[test]
fn two_cached_calls_release_shared_weights_after_the_last_idle_deadline() {
    runtime().block_on(async {
        let loads = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let pool = Arc::new(SharedModels::new(Arc::new(LeaseLoader {
            loads: Arc::clone(&loads),
            drops: Arc::clone(&drops),
        })));
        let mut config = config();
        config.call.idle_unload_minutes = 0;
        let (a, mut a_events) = VoiceCall::new(
            Box::new(FakeIo(Arc::default())),
            setup(&pool, config.clone()),
        );
        let (b, mut b_events) =
            VoiceCall::new(Box::new(FakeIo(Arc::default())), setup(&pool, config));
        let listening = |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Listening => Some(()),
            _ => None,
        };
        a.start(StartOptions::default());
        b.start(StartOptions::default());
        next(&mut a_events, listening).await;
        next(&mut b_events, listening).await;
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        a.stop();
        next(&mut a_events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Idle => Some(()),
            _ => None,
        })
        .await;
        assert_eq!(
            drops.load(Ordering::SeqCst),
            0,
            "B still leases the weights"
        );
        b.stop();
        until(|| drops.load(Ordering::SeqCst) == 1).await;
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        b.start(StartOptions::default());
        next(&mut b_events, listening).await;
        assert_eq!(
            loads.load(Ordering::SeqCst),
            2,
            "the retained call reloads after idle eviction"
        );
    });
}

#[test]
fn models_stay_while_the_call_runs_and_within_the_idle_minutes() {
    runtime().block_on(async {
        let models = FakeModels::default();
        let (loads, alive) = (Arc::clone(&models.loads), Arc::clone(&models.alive));
        let (call, mut events, _speakers) = call(models);
        call.start(StartOptions::default());
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Listening => Some(()),
            _ => None,
        })
        .await;
        call.stop();
        tokio::time::sleep(Duration::from_millis(300)).await;
        call.start(StartOptions::default());
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            (loads.load(Ordering::SeqCst), alive.load(Ordering::SeqCst)),
            (1, 1),
            "ten minutes by default"
        );
    });
}

/// An end-of-turn classifier that says every pause ends the turn, and counts how often it is asked.
struct Finished(Arc<AtomicUsize>);

#[async_trait]
impl EndOfTurnModel for Finished {
    async fn end_of_turn(&self, pcm: Vec<f32>, sample_rate: u32) -> Result<f32, String> {
        assert!(pcm.len() > 16_000 && sample_rate == 16_000);
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(0.9)
    }
}

#[test]
fn smart_turn_ends_a_turn_at_a_pause_its_model_says_is_the_end() {
    runtime().block_on(async {
        let asked = Arc::new(AtomicUsize::new(0));
        let models = FakeModels {
            end_of_turn: Some(Arc::new(Finished(Arc::clone(&asked)))),
            ..FakeModels::default()
        };
        let mut smart = fast();
        if let Some(input) = &mut smart.input {
            input.end_of_turn = crate::EndOfTurn::Smart {};
        }
        let (call, mut events, speakers) = with_config(models, smart);
        call.start(StartOptions::default());
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Listening => Some(()),
            _ => None,
        })
        .await;
        let sink = speakers.lock().unwrap().sink.clone().expect("started");
        // A pause of 1.2 s: shorter than any silence that ends a turn, longer than fast patience's 0.6 s pause.
        for frame in [silence(300), clip(QUILTER, 0.9), silence(1_200)]
            .concat()
            .chunks(160)
        {
            sink.send(IoEvent::Captured(frame.to_vec()));
        }
        let said = next(&mut events, |event| match event {
            VoiceEvent::Turn(TurnEvent::Finished { text, .. }) => Some(text),
            _ => None,
        })
        .await;
        assert!(!said.is_empty());
        assert!(asked.load(Ordering::SeqCst) >= 1);
    });
}

#[test]
fn dropping_the_call_closes_the_microphone_drops_the_models_and_ends_the_events() {
    runtime().block_on(async {
        let alive = Arc::new(AtomicUsize::new(0));
        let (call, mut events, speakers) = call(FakeModels {
            alive: Arc::clone(&alive),
            ..FakeModels::default()
        });
        call.start(StartOptions::default());
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Listening => Some(()),
            _ => None,
        })
        .await;
        let sink = speakers.lock().unwrap().sink.clone().expect("listening");
        // Something is being synthesized: a model task is under way when the owner goes.
        let _saying = call.say("Something the owner never hears.", SayOptions::default());
        drop(call);
        tokio::time::timeout(Duration::from_secs(5), async {
            while events.next().await.is_some() {}
        })
        .await
        .expect("the events end");
        assert!(speakers.lock().unwrap().sink.is_none(), "the io stopped");
        assert_eq!(alive.load(Ordering::SeqCst), 0, "the models were dropped");
        // What the microphone still delivers goes nowhere.
        sink.send(IoEvent::Captured(clip(QUILTER, 0.9)));
    });
}

#[test]
fn the_call_listens_only_once_its_microphone_and_speaker_are_ready() {
    runtime().block_on(async {
        let (call, mut events, speakers) = call(FakeModels::default());
        speakers.lock().unwrap().opening = true;
        call.start(StartOptions::default());
        until(|| speakers.lock().unwrap().sink.is_some()).await;
        // The permission prompt is still open: no state says it listens.
        let early = tokio::time::timeout(Duration::from_millis(200), async {
            loop {
                if let Some(VoiceEvent::State(state)) = events.next().await {
                    if state.listening != crate::Listening::Idle {
                        return;
                    }
                }
            }
        })
        .await;
        assert!(early.is_err(), "listening before the microphone was ready");
        let sink = speakers.lock().unwrap().sink.clone().unwrap();
        sink.send(IoEvent::Ready);
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Listening => Some(()),
            _ => None,
        })
        .await;

        // A microphone refused while opening is an error, and the call never listened.
        call.stop();
        until(|| speakers.lock().unwrap().sink.is_none()).await;
        call.start(StartOptions::default());
        until(|| speakers.lock().unwrap().sink.is_some()).await;
        let sink = speakers.lock().unwrap().sink.clone().unwrap();
        sink.send(IoEvent::Failed("microphone-denied".into()));
        let code = next(&mut events, |event| match event {
            VoiceEvent::Error(error) => Some(error.code),
            VoiceEvent::State(state) if state.listening == crate::Listening::Listening => {
                Some("listened".into())
            }
            _ => None,
        })
        .await;
        assert_eq!(code, "microphone-denied");
        until(|| speakers.lock().unwrap().sink.is_none()).await;
    });
}

#[test]
fn a_live_update_swaps_models_at_the_next_turn_without_restarting() {
    runtime().block_on(async {
        let (call, mut events, speakers) = call(FakeModels::default());
        call.start(StartOptions::default());
        assert_eq!(started(&mut events).await, crate::Listening::Listening);
        // Silence → smart, on models that end turns: they load while the call listens on, and take over between turns.
        let asked = Arc::new(AtomicUsize::new(0));
        let ending = Arc::new(FakeModels {
            end_of_turn: Some(Arc::new(Finished(Arc::clone(&asked)))),
            ..FakeModels::default()
        });
        let mut smart = fast();
        if let Some(input) = &mut smart.input {
            input.end_of_turn = crate::EndOfTurn::Smart {};
        }
        call.update(setup(&ending, smart));
        until(|| ending.classifiers.load(Ordering::SeqCst) == 1).await;
        let sink = speakers
            .lock()
            .unwrap()
            .sink
            .clone()
            .expect("still the same io");
        // A pause of 1.2 s: shorter than any silence that ends a turn; only the new end-of-turn model ends it.
        for frame in [silence(300), clip(QUILTER, 0.9), silence(1_200)]
            .concat()
            .chunks(160)
        {
            sink.send(IoEvent::Captured(frame.to_vec()));
        }
        let restarted = next(&mut events, |event| match event {
            VoiceEvent::Turn(TurnEvent::Finished { .. }) => Some(false),
            VoiceEvent::State(state) if state.listening == crate::Listening::Idle => Some(true),
            _ => None,
        })
        .await;
        assert!(!restarted, "the call never stopped");
        assert!(asked.load(Ordering::SeqCst) >= 1);
        assert_eq!(ending.loads.load(Ordering::SeqCst), 1);
    });
}
#[test]
fn every_stop_is_answered_with_an_idle_state() {
    runtime().block_on(async {
        let (call, mut events, _speakers) = call(FakeModels::default());
        // Never started: the stop still says idle.
        call.stop();
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Idle => Some(()),
            _ => None,
        })
        .await;
        // Stopped twice: each stop is answered.
        call.stop();
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Idle => Some(()),
            _ => None,
        })
        .await;
    });
}

/// A call listening on a detector that has stopped answering, the task stuck in it.
async fn stuck(models: FakeModels) -> (VoiceCall, Events, Arc<Mutex<Speakers>>) {
    let hang = Arc::clone(&models.hang);
    let (call, mut events, speakers) = call(models);
    call.start(StartOptions::default());
    next(&mut events, |event| match event {
        VoiceEvent::State(state) if state.listening == crate::Listening::Listening => Some(()),
        _ => None,
    })
    .await;
    hang.store(true, Ordering::SeqCst);
    let sink = speakers.lock().unwrap().sink.clone().unwrap();
    sink.send(IoEvent::Captured(vec![0.0; 1_600]));
    tokio::time::sleep(Duration::from_millis(100)).await;
    (call, events, speakers)
}

#[test]
fn dropping_the_call_ends_it_while_its_detector_never_answers() {
    runtime().block_on(async {
        let alive = Arc::new(AtomicUsize::new(0));
        let (call, mut events, speakers) = stuck(FakeModels {
            alive: Arc::clone(&alive),
            ..FakeModels::default()
        })
        .await;
        drop(call);
        tokio::time::timeout(Duration::from_secs(5), async {
            while events.next().await.is_some() {}
        })
        .await
        .expect("the events end though the detector never answered");
        assert!(speakers.lock().unwrap().sink.is_none(), "the io stopped");
        assert_eq!(alive.load(Ordering::SeqCst), 0, "the models were dropped");
    });
}

#[test]
fn a_stop_ends_the_call_while_its_detector_never_answers_and_it_starts_again() {
    runtime().block_on(async {
        let models = FakeModels::default();
        let (loads, hang) = (Arc::clone(&models.loads), Arc::clone(&models.hang));
        let (call, mut events, speakers) = stuck(models).await;
        call.stop();
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Idle => Some(()),
            _ => None,
        })
        .await;
        assert!(speakers.lock().unwrap().sink.is_none(), "the io stopped");
        // The models were dropped with the detector: the next start loads them again, and listens.
        hang.store(false, Ordering::SeqCst);
        call.start(StartOptions::default());
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Listening => Some(()),
            _ => None,
        })
        .await;
        assert_eq!(loads.load(Ordering::SeqCst), 2);
    });
}

#[test]
fn a_handle_cancels_what_it_says_and_tells_how_it_ended() {
    runtime().block_on(async {
        let (call, mut events, speakers) = call(FakeModels::default());
        call.start(StartOptions::default());
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Listening => Some(()),
            _ => None,
        })
        .await;
        // The person speaks: a say delivered locally after the turn opens is settled immediately.
        let sink = speakers.lock().unwrap().sink.clone().unwrap();
        for frame in [silence(300), clip(QUILTER, 0.9)].concat().chunks(160) {
            sink.send(IoEvent::Captured(frame.to_vec()));
        }
        next(&mut events, |event| match event {
            VoiceEvent::Turn(TurnEvent::Started { .. }) => Some(()),
            _ => None,
        })
        .await;
        let mut saying = call.say("Cancelled while it waits.", SayOptions::default());
        saying.cancel();
        let mut steps = Vec::new();
        while let Some(step) = tokio::time::timeout(Duration::from_secs(5), saying.next())
            .await
            .expect("in time")
        {
            steps.push(step);
        }
        assert_eq!(
            steps,
            [SayEvent::Done {
                outcome: SayOutcome::NotPlayed {
                    reason: crate::StopReason::BargeIn
                }
            }]
        );
        // Said while stopped: not played.
        call.stop();
        let mut stopped = call.say("Said while stopped.", SayOptions::default());
        let step = tokio::time::timeout(Duration::from_secs(5), stopped.next())
            .await
            .expect("in time");
        assert_eq!(
            step,
            Some(SayEvent::Done {
                outcome: SayOutcome::NotPlayed {
                    reason: crate::StopReason::Stopped
                }
            })
        );
    });
}

/// The next take, or `None` once the call says it stopped with no take before it.
async fn take_or_idle(events: &mut Events) -> Option<crate::Take> {
    next(events, |event| match event {
        VoiceEvent::Take(take) => Some(Some(take)),
        VoiceEvent::State(state) if state.listening == crate::Listening::Idle => Some(None),
        _ => None,
    })
    .await
}

#[test]
fn a_start_that_asks_for_a_take_hands_it_over_before_it_says_it_stopped() {
    runtime().block_on(async {
        let (call, mut events, speakers) = call(FakeModels::default());
        call.start(StartOptions { take: true });
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Listening => Some(()),
            _ => None,
        })
        .await;
        let sink = speakers.lock().unwrap().sink.clone().expect("started");
        let heard = [silence(320), clip(QUILTER, 0.9), silence(2_600)].concat();
        for frame in heard.chunks(160) {
            sink.send(IoEvent::Captured(frame.to_vec()));
        }
        let turn_id = next(&mut events, |event| match event {
            VoiceEvent::Turn(TurnEvent::Finished { turn_id, .. }) => Some(turn_id),
            _ => None,
        })
        .await;
        // Muted, the take keeps silence (told after the samples, on the same channel: they were taken by then).
        call.mute(true);
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Muted => Some(()),
            _ => None,
        })
        .await;
        sink.send(IoEvent::Captured(vec![0.5; 1_600]));
        sink.send(IoEvent::MicrophoneMuted(true));
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.microphone == crate::Microphone::Muted => Some(()),
            _ => None,
        })
        .await;
        call.stop();
        let take = take_or_idle(&mut events)
            .await
            .expect("the take, before the idle state");
        assert_eq!(take.sample_rate, 16_000);
        assert_eq!(take.samples.len(), heard.len() + 1_600);
        assert_eq!(&take.samples[..heard.len()], &heard[..]);
        assert!(take.samples[heard.len()..].iter().all(|&s| s == 0.0));
        assert_eq!(
            take.ended_at - take.started_at,
            (heard.len() as u64 + 1_600) / 16
        );
        assert_eq!(take.turns.len(), 1, "{:?}", take.turns);
        let turn = &take.turns[0];
        assert_eq!(turn.turn_id, turn_id);
        assert!(turn.speech_ms > 320 && turn.end_ms.is_some_and(|end| end > turn.speech_ms));
        assert_eq!(
            take_or_idle(&mut events).await,
            None,
            "one take, then the stop"
        );

        // Started again with no take, it keeps none.
        call.mute(false);
        call.start(StartOptions::default());
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Listening => Some(()),
            _ => None,
        })
        .await;
        call.stop();
        assert_eq!(take_or_idle(&mut events).await, None);
    });
}

#[test]
fn a_take_ends_before_the_failure_that_ends_its_listening() {
    runtime().block_on(async {
        let (call, mut events, speakers) = call(FakeModels::default());
        call.start(StartOptions { take: true });
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Listening => Some(()),
            _ => None,
        })
        .await;
        let sink = speakers.lock().unwrap().sink.clone().expect("started");
        sink.send(IoEvent::Captured(silence(100)));
        sink.send(IoEvent::Failed("microphone-lost".into()));
        let first = next(&mut events, |event| match event {
            VoiceEvent::Take(take) => Some(Ok(take.samples.len())),
            VoiceEvent::Error(error) => Some(Err(error.code)),
            _ => None,
        })
        .await;
        assert_eq!(first, Ok(1_600));
        let code = next(&mut events, |event| match event {
            VoiceEvent::Error(error) => Some(error.code),
            _ => None,
        })
        .await;
        assert_eq!(code, "microphone-lost");
    });
}

#[test]
fn a_live_update_that_cannot_load_leaves_the_call_as_it_was() {
    runtime().block_on(async {
        let models = Arc::new(FakeModels::default());
        let (call, mut events, speakers) = with_setup(setup(&models, fast()));
        call.start(StartOptions { take: true });
        assert_eq!(started(&mut events).await, crate::Listening::Listening);
        // Smart on models with no end-of-turn model: the change fails, and the call goes on listening with silence.
        let mut smart = fast();
        if let Some(input) = &mut smart.input {
            input.end_of_turn = crate::EndOfTurn::Smart {};
        }
        call.update(setup(&models, smart));
        let seen = next(&mut events, |event| match event {
            VoiceEvent::Error(error) => Some(error.code),
            VoiceEvent::Take(_) => Some("take".into()),
            VoiceEvent::State(state) if state.listening == crate::Listening::Idle => {
                Some("idle".into())
            }
            _ => None,
        })
        .await;
        assert_eq!(seen, "end-of-turn-missing");
        let sink = speakers.lock().unwrap().sink.clone().unwrap();
        for frame in [silence(300), clip(QUILTER, 0.9), silence(2_600)]
            .concat()
            .chunks(160)
        {
            sink.send(IoEvent::Captured(frame.to_vec()));
        }
        next(&mut events, |event| match event {
            VoiceEvent::Turn(TurnEvent::Finished { .. }) => Some(()),
            VoiceEvent::State(state) if state.listening == crate::Listening::Idle => {
                panic!("the call stopped")
            }
            _ => None,
        })
        .await;
    });
}
/// Starts `call` and sends it `audio` as its microphone; answers its events.
async fn hear(
    call: &VoiceCall,
    events: &mut Events,
    speakers: &Arc<Mutex<Speakers>>,
    audio: Vec<f32>,
) {
    call.start(StartOptions::default());
    next(events, |event| match event {
        VoiceEvent::State(state) if state.listening == crate::Listening::Listening => Some(()),
        _ => None,
    })
    .await;
    let sink = speakers.lock().unwrap().sink.clone().expect("started");
    for frame in audio.chunks(160) {
        sink.send(IoEvent::Captured(frame.to_vec()));
    }
}

#[test]
fn end_input_closes_the_open_turn_without_waiting_for_its_silence() {
    runtime().block_on(async {
        let (call, mut events, speakers) = call(FakeModels::default());
        // Nothing is open: it resolves and nothing happens.
        call.end_input().await;
        // Speech with no silence after it: the turn is open and would wait for its endpoint.
        hear(
            &call,
            &mut events,
            &speakers,
            [silence(300), clip(QUILTER, 0.9)].concat(),
        )
        .await;
        next(&mut events, |event| match event {
            VoiceEvent::Turn(TurnEvent::Started { .. }) => Some(()),
            _ => None,
        })
        .await;
        call.end_input().await;
        call.end_input().await;
        let said = next(&mut events, |event| match event {
            VoiceEvent::Turn(TurnEvent::Finished { text, .. }) => Some(text),
            _ => None,
        })
        .await;
        assert!(!said.is_empty());
        // Exactly one: the second call found nothing open.
        let another = tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                if let Some(VoiceEvent::Turn(TurnEvent::Finished { .. })) = events.next().await {
                    return;
                }
            }
        })
        .await;
        assert!(another.is_err(), "one finished per turn");
    });
}

#[test]
fn end_input_of_one_call_leaves_another_calls_turn_open() {
    runtime().block_on(async {
        let (first, mut first_events, first_speakers) = call(FakeModels::default());
        let (second, mut second_events, second_speakers) = call(FakeModels::default());
        let audio = [silence(300), clip(QUILTER, 0.9)].concat();
        hear(&first, &mut first_events, &first_speakers, audio.clone()).await;
        hear(&second, &mut second_events, &second_speakers, audio).await;
        for events in [&mut first_events, &mut second_events] {
            next(events, |event| match event {
                VoiceEvent::Turn(TurnEvent::Started { .. }) => Some(()),
                _ => None,
            })
            .await;
        }
        first.end_input().await;
        next(&mut first_events, |event| match event {
            VoiceEvent::Turn(TurnEvent::Finished { .. }) => Some(()),
            _ => None,
        })
        .await;
        let other = tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                if let Some(VoiceEvent::Turn(TurnEvent::Finished { .. })) =
                    second_events.next().await
                {
                    return;
                }
            }
        })
        .await;
        assert!(other.is_err(), "the other call's turn is still open");
    });
}

/// How what `saying` says ends, within five seconds.
async fn outcome(mut saying: crate::Saying) -> SayOutcome {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(SayEvent::Done { outcome }) = saying.next().await {
                return outcome;
            }
        }
    })
    .await
    .expect("it ended in time")
}

/// The first state that says the call is started.
async fn started(events: &mut Events) -> crate::Listening {
    next(events, |event| match event {
        VoiceEvent::State(state) if state.listening != crate::Listening::Idle => {
            Some(state.listening)
        }
        VoiceEvent::Error(error) => panic!("{}", error.code),
        _ => None,
    })
    .await
}

#[test]
fn a_call_with_only_an_output_half_loads_and_opens_nothing_of_the_input() {
    runtime().block_on(async {
        let models = Arc::new(FakeModels::default());
        let (call, mut events, speakers) = with_setup(setup(
            &models,
            CallConfig {
                input: None,
                ..fast()
            },
        ));
        call.start(StartOptions::default());
        assert_eq!(started(&mut events).await, crate::Listening::Off);
        assert!(!speakers.lock().unwrap().input, "no microphone asked for");
        assert_eq!(
            [&models.loads, &models.transcribers, &models.classifiers]
                .map(|loads| loads.load(Ordering::SeqCst)),
            [0, 0, 0],
            "no detector, transcriber or end-of-turn model"
        );
        assert_eq!(models.speakers.load(Ordering::SeqCst), 1);
        let saying = call.say("Only spoken.", SayOptions::default());
        assert_eq!(outcome(saying).await, SayOutcome::Heard);
    });
}

#[test]
fn a_transcription_try_loads_no_speaker_and_silence_loads_no_end_of_turn_model() {
    runtime().block_on(async {
        let models = Arc::new(FakeModels::default());
        let try_config = CallConfig {
            output: None,
            ..config_with(|input| {
                input.end_of_turn = crate::EndOfTurn::Silence { ms: Some(1_500) };
            })
        };
        let (call, mut events, speakers) = with_setup(setup(&models, try_config));
        call.start(StartOptions::default());
        assert_eq!(started(&mut events).await, crate::Listening::Listening);
        assert!(speakers.lock().unwrap().input);
        assert_eq!(
            [
                &models.loads,
                &models.transcribers,
                &models.speakers,
                &models.classifiers
            ]
            .map(|loads| loads.load(Ordering::SeqCst)),
            [1, 1, 0, 0]
        );
        let saying = call.say("Nobody hears this.", SayOptions::default());
        assert!(matches!(
            outcome(saying).await,
            SayOutcome::NotPlayed {
                reason: crate::StopReason::Failed { ref code }
            } if code == "output-missing"
        ));
    });
}

#[test]
fn an_update_keeps_what_the_same_models_loaded() {
    runtime().block_on(async {
        let models = Arc::new(FakeModels {
            end_of_turn: Some(Arc::new(Finished(Arc::default()))),
            ..FakeModels::default()
        });
        let try_config = CallConfig {
            output: None,
            ..fast()
        };
        let (call, mut events, _speakers) = with_setup(setup(&models, try_config));
        call.start(StartOptions::default());
        started(&mut events).await;
        // The output half joins on the same models: only the speaker loads, and what is said is heard.
        call.update(setup(&models, fast()));
        let saying = call.say("Now it speaks.", SayOptions::default());
        assert_eq!(outcome(saying).await, SayOutcome::Heard);
        // The end of turn turns smart and back: only the end-of-turn model loads, once.
        let smart = config_with(|input| {
            input.patience = Patience::Fast;
            input.end_of_turn = crate::EndOfTurn::Smart {};
        });
        call.update(setup(
            &models,
            CallConfig {
                call: fast().call,
                ..smart
            },
        ));
        until(|| models.classifiers.load(Ordering::SeqCst) == 1).await;
        call.update(setup(&models, fast()));
        call.stop();
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Idle => Some(()),
            _ => None,
        })
        .await;
        assert_eq!(
            [
                &models.loads,
                &models.transcribers,
                &models.speakers,
                &models.classifiers
            ]
            .map(|loads| loads.load(Ordering::SeqCst)),
            [1, 1, 1, 1]
        );
    });
}

#[test]
fn an_output_change_waits_for_the_next_utterance_and_never_cuts_what_plays() {
    runtime().block_on(async {
        let models = Arc::new(FakeModels::default());
        let speaking = CallConfig {
            input: None,
            ..fast()
        };
        let (call, mut events, speakers) = with_setup(setup(&models, speaking.clone()));
        speakers.lock().unwrap().held = true;
        call.start(StartOptions::default());
        started(&mut events).await;
        let first = call.say("The first thing said.", SayOptions::default());
        until(|| speakers.lock().unwrap().played.len() == 1).await;
        // Another voice while it plays: nothing stops, and what waits behind it waits for the change.
        let mut other = speaking;
        other.output = Some(crate::OutputConfig {
            voice: Some("other".into()),
            ..crate::OutputConfig::default()
        });
        call.update(setup(&models, other));
        let second = call.say("The second thing said.", SayOptions::default());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(speakers.lock().unwrap().stopped, 0, "nothing was cut");
        assert_eq!(models.voices.lock().unwrap().len(), 1, "the next waits");
        let sink = speakers.lock().unwrap().sink.clone().unwrap();
        let (utterance, chunk) = {
            let played = &speakers.lock().unwrap().played[0];
            (played.0.clone(), played.1)
        };
        speakers.lock().unwrap().held = false;
        sink.send(IoEvent::ChunkStarted {
            utterance: utterance.clone(),
            chunk,
        });
        sink.send(IoEvent::ChunkPlayed { utterance, chunk });
        assert_eq!(outcome(first).await, SayOutcome::Heard);
        assert_eq!(outcome(second).await, SayOutcome::Heard);
        assert_eq!(
            *models.voices.lock().unwrap(),
            [None, Some("other".into())],
            "the first in its voice, the next in the new one"
        );
        assert_eq!(
            models.speakers.load(Ordering::SeqCst),
            1,
            "the same speaker"
        );
        assert_eq!(speakers.lock().unwrap().stopped, 0);
    });
}

#[test]
fn an_input_half_added_later_opens_the_microphone_and_listens() {
    runtime().block_on(async {
        let models = Arc::new(FakeModels::default());
        let speaking = CallConfig {
            input: None,
            ..fast()
        };
        let (call, mut events, speakers) = with_setup(setup(&models, speaking));
        call.start(StartOptions::default());
        assert_eq!(started(&mut events).await, crate::Listening::Off);
        assert!(!speakers.lock().unwrap().input);
        // A microphone appears: the input half joins without a restart.
        call.update(setup(&models, fast()));
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Listening => Some(()),
            VoiceEvent::State(state) if state.listening == crate::Listening::Idle => {
                panic!("restarted")
            }
            _ => None,
        })
        .await;
        assert!(speakers.lock().unwrap().input, "the microphone was opened");
        assert_eq!(models.loads.load(Ordering::SeqCst), 1);
        let sink = speakers.lock().unwrap().sink.clone().unwrap();
        for frame in [silence(300), clip(QUILTER, 0.9), silence(2_600)]
            .concat()
            .chunks(160)
        {
            sink.send(IoEvent::Captured(frame.to_vec()));
        }
        next(&mut events, |event| match event {
            VoiceEvent::Turn(TurnEvent::Finished { .. }) => Some(()),
            _ => None,
        })
        .await;
        // And it can go again, at the next turn: the microphone closes.
        call.update(setup(
            &models,
            CallConfig {
                input: None,
                ..fast()
            },
        ));
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Off => Some(()),
            _ => None,
        })
        .await;
        assert!(!speakers.lock().unwrap().input, "the microphone was closed");
    });
}

#[test]
fn a_lost_microphone_leaves_a_call_that_speaks_speaking() {
    runtime().block_on(async {
        let (call, mut events, speakers) = call(FakeModels::default());
        call.start(StartOptions::default());
        assert_eq!(started(&mut events).await, crate::Listening::Listening);
        let sink = speakers.lock().unwrap().sink.clone().unwrap();
        sink.send(IoEvent::InputLost("microphone-lost".into()));
        let code = next(&mut events, |event| match event {
            VoiceEvent::Error(error) => Some(error.code),
            _ => None,
        })
        .await;
        assert_eq!(code, "microphone-lost");
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Off => Some(()),
            VoiceEvent::State(state) if state.listening == crate::Listening::Idle => {
                panic!("the call stopped")
            }
            _ => None,
        })
        .await;
        let saying = call.say("Still speaking.", SayOptions::default());
        assert_eq!(outcome(saying).await, SayOutcome::Heard);
        // A microphone refused while opening: it starts speaking only.
        call.stop();
        until(|| speakers.lock().unwrap().sink.is_none()).await;
        call.update(setup(&Arc::new(FakeModels::default()), fast()));
        speakers.lock().unwrap().opening = true;
        call.start(StartOptions::default());
        until(|| speakers.lock().unwrap().sink.is_some()).await;
        let sink = speakers.lock().unwrap().sink.clone().unwrap();
        sink.send(IoEvent::InputLost("microphone-denied".into()));
        sink.send(IoEvent::Ready);
        // The call works, speaking only, and then says why it does not listen.
        let first = next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Off => Some(None),
            VoiceEvent::State(state) if state.listening == crate::Listening::Listening => {
                panic!("listening without a microphone")
            }
            VoiceEvent::Error(error) => Some(Some(error.code)),
            _ => None,
        })
        .await;
        assert_eq!(first, None, "the state before the error");
        let code = next(&mut events, |event| match event {
            VoiceEvent::Error(error) => Some(error.code),
            _ => None,
        })
        .await;
        assert_eq!(code, "microphone-denied");
    });
}

#[test]
fn a_lost_microphone_ends_a_call_that_only_listens() {
    runtime().block_on(async {
        let listening = CallConfig {
            output: None,
            ..fast()
        };
        let (call, mut events, speakers) = with_config(FakeModels::default(), listening);
        call.start(StartOptions::default());
        started(&mut events).await;
        let sink = speakers.lock().unwrap().sink.clone().unwrap();
        sink.send(IoEvent::InputLost("microphone-lost".into()));
        let code = next(&mut events, |event| match event {
            VoiceEvent::Error(error) => Some(error.code),
            _ => None,
        })
        .await;
        assert_eq!(code, "microphone-lost");
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Idle => Some(()),
            _ => None,
        })
        .await;
        drop(call);
    });
}
