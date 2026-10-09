//! The call's task on fakes: models that detect on energy, transcribe to a fixed text and speak silence, and a
//! microphone and speaker the test plays by hand, on a Tokio runtime as an app runs it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;

use super::{Events, VoiceCall};
use crate::config::{Patience, VoiceConfig};
use crate::io::{AudioIo, IoEvent, IoSink};
use crate::models::{Detector, Loaded, Models, Speaker, Transcriber};
use crate::room::{PlaybackStatus, Reply, RoomEvent, RoomMessage, TurnPhase};
use crate::test_support::{clip, config, silence, EnergyVad, QUILTER, WINDOW};
use crate::VoiceEvent;

struct FakeModels {
    fail: Option<&'static str>,
}

#[async_trait]
impl Models for FakeModels {
    async fn load(&self, _config: &VoiceConfig) -> Result<Loaded, String> {
        if let Some(code) = self.fail {
            return Err(code.into());
        }
        Ok(Loaded {
            detector: Box::new(FakeDetector {
                vad: EnergyVad::new(),
                pending: Vec::new(),
                end: 0,
            }),
            transcriber: Arc::new(FakeTranscriber),
            speaker: Arc::new(FakeSpeaker),
        })
    }
}

struct FakeDetector {
    vad: EnergyVad,
    pending: Vec<f32>,
    end: u64,
}

#[async_trait]
impl Detector for FakeDetector {
    async fn accept(&mut self, pcm: &[f32]) -> Result<Vec<(u64, bool)>, String> {
        self.pending.extend_from_slice(pcm);
        let whole = self.pending.len() / WINDOW * WINDOW;
        let windows: Vec<f32> = self.pending.drain(..whole).collect();
        Ok(windows
            .chunks(WINDOW)
            .map(|window| {
                self.end += WINDOW as u64;
                (self.end, self.vad.window(window))
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
    async fn transcribe(&self, pcm: Vec<f32>, language: Option<String>) -> Result<String, String> {
        assert!(pcm.len() > 16_000);
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
        _language: Option<String>,
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
    let speakers = Arc::default();
    let config = VoiceConfig {
        patience: Patience::Fast,
        audio_grace_ms: 0,
        ..config()
    };
    let (call, events) = VoiceCall::with_models(
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
        let (call, mut events, speakers) = call(FakeModels { fail: None });
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
fn smart_turn_is_refused_until_the_engine_has_it() {
    runtime().block_on(async {
        let (call, mut events, _speakers) = call(FakeModels { fail: None });
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
        assert_eq!(code, "end-of-turn-unavailable");
    });
}
