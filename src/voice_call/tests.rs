//! The call's task on fakes: models that detect on energy, transcribe to a fixed text and speak silence, and a
//! microphone and speaker the test plays by hand, on a Tokio runtime as an app runs it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;

use super::{Events, VoiceCall};
use crate::config::{Patience, VoiceConfig};
use crate::io::{AudioIo, IoEvent, IoSink};
use crate::models::{EndOfTurnModel, Models, Speaker, Transcriber, Vad, VadFrame, VoiceModels};
use crate::room::{PlaybackStatus, Reply, RoomEvent, RoomMessage, TurnPhase};
use crate::test_support::{clip, config, silence, EnergyVad, QUILTER, WINDOW};
use crate::VoiceEvent;

#[derive(Default)]
struct FakeModels {
    fail: Option<&'static str>,
    /// The end-of-turn classifier it supplies, if any.
    end_of_turn: Option<Arc<dyn EndOfTurnModel>>,
    /// How many times the models were loaded, and how many detectors are alive (one per load not dropped yet).
    loads: Arc<AtomicUsize>,
    alive: Arc<AtomicUsize>,
}

#[async_trait]
impl VoiceModels for FakeModels {
    async fn load(&self) -> Result<Models, String> {
        if let Some(code) = self.fail {
            return Err(code.into());
        }
        self.loads.fetch_add(1, Ordering::SeqCst);
        self.alive.fetch_add(1, Ordering::SeqCst);
        Ok(Models {
            vad: Box::new(FakeDetector {
                vad: EnergyVad::new(),
                pending: Vec::new(),
                end: 0,
                alive: Arc::clone(&self.alive),
            }),
            transcriber: Arc::new(FakeTranscriber),
            speaker: Arc::new(FakeSpeaker),
            end_of_turn: self.end_of_turn.clone(),
        })
    }
}

struct FakeDetector {
    vad: EnergyVad,
    pending: Vec<f32>,
    end: u64,
    alive: Arc<AtomicUsize>,
}

impl Drop for FakeDetector {
    fn drop(&mut self) {
        self.alive.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl Vad for FakeDetector {
    async fn accept(&mut self, pcm: &[f32]) -> Result<Vec<VadFrame>, String> {
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

struct FakeSpeaker;

#[async_trait]
impl Speaker for FakeSpeaker {
    async fn speak(
        &self,
        text: String,
        _voice: Option<String>,
        _language: Option<String>,
        _speed: f32,
    ) -> Result<(Vec<f32>, u32), String> {
        Ok((vec![0.0; text.len() * 10], 24_000))
    }
}

/// What the call did to the speaker, and the sink it was given.
#[derive(Default)]
struct Speakers {
    sink: Option<IoSink>,
    played: Vec<(String, usize, usize, u32)>,
    stopped: usize,
}

struct FakeIo(Arc<Mutex<Speakers>>);

impl AudioIo for FakeIo {
    fn start(&mut self, sink: IoSink) -> Result<(), String> {
        self.0.lock().unwrap().sink = Some(sink);
        Ok(())
    }

    fn play(&mut self, utterance: &str, chunk: usize, samples: Vec<f32>, sample_rate: u32) {
        let mut speakers = self.0.lock().unwrap();
        speakers
            .played
            .push((utterance.into(), chunk, samples.len(), sample_rate));
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

fn call(models: FakeModels) -> (VoiceCall, Events, Arc<Mutex<Speakers>>) {
    with_config(
        models,
        VoiceConfig {
            patience: Patience::Fast,
            audio_grace_ms: 0,
            ..config()
        },
    )
}

fn with_config(
    models: FakeModels,
    config: VoiceConfig,
) -> (VoiceCall, Events, Arc<Mutex<Speakers>>) {
    let speakers = Arc::default();
    let (call, events) = VoiceCall::new(
        Arc::new(models),
        Box::new(FakeIo(Arc::clone(&speakers))),
        config,
    );
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
        call.start();
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
        let turn = next(&mut events, |event| match event {
            VoiceEvent::RoomMessage(RoomMessage::UserTurn(turn))
                if turn.phase == TurnPhase::Finished =>
            {
                Some(turn)
            }
            _ => None,
        })
        .await;
        assert_eq!(
            turn.text.as_deref(),
            Some("Mister Quilter is the apostle of the middle classes.")
        );

        call.room_event(RoomEvent::Reply(Reply {
            utterance_id: "u1".into(),
            revision: 1,
            reply_revision: 2,
            thread_id: "t".into(),
            history_id: "h".into(),
            text: "Noted. I will read the apostle's gospel tonight.".into(),
            language: Some("en".into()),
            replay: false,
        }));
        let heard = next(&mut events, |event| match event {
            VoiceEvent::RoomMessage(RoomMessage::Playback(report))
                if report.status == PlaybackStatus::Heard =>
            {
                Some(report)
            }
            _ => None,
        })
        .await;
        assert_eq!(heard.heard_chars, 48);
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
        call.start();
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
        let (call, mut events, _speakers) = call(FakeModels::default());
        call.set_config(VoiceConfig {
            end_of_turn: crate::EndOfTurn::SmartTurn,
            ..config()
        });
        call.start();
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
        let (call, mut events, _speakers) = with_config(
            models,
            VoiceConfig {
                idle_unload_minutes: 0,
                ..config()
            },
        );
        call.start();
        next(&mut events, listening).await;
        assert_eq!(
            (loads.load(Ordering::SeqCst), alive.load(Ordering::SeqCst)),
            (1, 1)
        );
        call.stop();
        until(|| alive.load(Ordering::SeqCst) == 0).await;
        call.start();
        next(&mut events, listening).await;
        assert_eq!(
            (loads.load(Ordering::SeqCst), alive.load(Ordering::SeqCst)),
            (2, 1),
            "loaded again"
        );
    });
}

#[test]
fn models_stay_while_the_call_runs_and_within_the_idle_minutes() {
    runtime().block_on(async {
        let models = FakeModels::default();
        let (loads, alive) = (Arc::clone(&models.loads), Arc::clone(&models.alive));
        let (call, mut events, _speakers) = call(models);
        call.start();
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == crate::Listening::Listening => Some(()),
            _ => None,
        })
        .await;
        call.stop();
        tokio::time::sleep(Duration::from_millis(300)).await;
        call.start();
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
        let (call, mut events, speakers) = with_config(
            models,
            VoiceConfig {
                end_of_turn: crate::EndOfTurn::SmartTurn,
                patience: Patience::Fast,
                audio_grace_ms: 0,
                ..config()
            },
        );
        call.start();
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
        let turn = next(&mut events, |event| match event {
            VoiceEvent::RoomMessage(RoomMessage::UserTurn(turn))
                if turn.phase == TurnPhase::Finished =>
            {
                Some(turn)
            }
            _ => None,
        })
        .await;
        assert!(turn.text.is_some());
        assert!(asked.load(Ordering::SeqCst) >= 1);
    });
}
