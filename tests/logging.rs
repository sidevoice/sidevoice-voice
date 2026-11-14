//! The canary: a call on fakes whose models say and fail with canary text, under a global subscriber and a
//! multi-threaded runtime, so every thread is captured. What the call logs must show that its operations ran and must
//! hold none of the canaries. One test: the global subscriber is the process's.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use sidevoice_voice::{
    async_trait, AudioIo, EndOfTurnModel, Events, InputModels, IoEvent, IoSink, Listening,
    OutputModels, SayOptions, Speaker, StartOptions, Transcriber, TurnEvent, Vad, VadFrame,
    VoiceCall, VoiceConfig, VoiceEvent, VoiceInput, VoiceOutput, VoiceSetup,
};
use sidevoice_voice::{InputConfig, OutputConfig, Patience};

const WINDOW: usize = 512;
/// What the transcriber returns for the first turn, and fails with for the second.
const TRANSCRIPT: &str = "canary transcript 7f3a";
const FAILURE: &str = "Canary sentence 7f3a: could not open /home/canary-user-7f3a";
/// What the call is asked to say.
const SAID: &str = "canary words 7f3a, said aloud.";
const SAID_FAILING: &str = "canary words 7f3a, fail aloud.";

/// What a subscriber wrote, shared with the test that reads it.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;

    fn make_writer(&'a self) -> Captured {
        self.clone()
    }
}

/// Speech wherever a window is loud.
struct Loud {
    pending: Vec<f32>,
    end: u64,
}

#[async_trait]
impl Vad for Loud {
    async fn accept(&mut self, pcm: &[f32]) -> Result<Vec<VadFrame>, String> {
        self.pending.extend_from_slice(pcm);
        let whole = self.pending.len() / WINDOW * WINDOW;
        let windows: Vec<f32> = self.pending.drain(..whole).collect();
        Ok(windows
            .chunks(WINDOW)
            .map(|window| {
                self.end += WINDOW as u64;
                let energy = window.iter().map(|s| s * s).sum::<f32>() / window.len() as f32;
                VadFrame {
                    end: self.end,
                    speech: energy > 0.04,
                    probability: None,
                }
            })
            .collect())
    }

    async fn reset(&mut self) {
        self.pending.clear();
        self.end = 0;
    }
}

/// Transcribes the first turn, and fails the others with a sentence that holds a path.
struct Words(AtomicUsize);

#[async_trait]
impl Transcriber for Words {
    async fn transcribe(
        &self,
        _pcm: Vec<f32>,
        _sample_rate: u32,
        _language: Option<String>,
    ) -> Result<String, String> {
        match self.0.fetch_add(1, Ordering::SeqCst) {
            0 => Ok(TRANSCRIPT.into()),
            _ => Err(FAILURE.into()),
        }
    }
}

/// Speaks silence, and fails on the text that says to.
struct Voice;

#[async_trait]
impl Speaker for Voice {
    async fn speak(
        &self,
        text: String,
        _voice: Option<String>,
        _language: Option<String>,
        _speed: f32,
    ) -> Result<(Vec<f32>, u32), String> {
        if text.contains("fail") {
            return Err(FAILURE.into());
        }
        Ok((vec![0.0; 2_400], 24_000))
    }
}

struct Models(Arc<Words>);

#[async_trait]
impl InputModels for Models {
    async fn vad(&self) -> Result<Box<dyn Vad>, String> {
        Ok(Box::new(Loud {
            pending: Vec::new(),
            end: 0,
        }))
    }

    async fn transcriber(&self) -> Result<Arc<dyn Transcriber>, String> {
        Ok(self.0.clone())
    }

    async fn end_of_turn(&self) -> Result<Arc<dyn EndOfTurnModel>, String> {
        Err("end-of-turn-missing".into())
    }
}

#[async_trait]
impl OutputModels for Models {
    async fn speaker(&self) -> Result<Arc<dyn Speaker>, String> {
        Ok(Arc::new(Voice))
    }
}

/// A microphone and speaker the test plays by hand.
struct Io(Arc<Mutex<Option<IoSink>>>);

impl AudioIo for Io {
    fn start(&mut self, sink: IoSink, _input: bool) -> Result<(), String> {
        sink.send(IoEvent::Ready);
        *self.0.lock().unwrap() = Some(sink);
        Ok(())
    }

    fn set_input(&mut self, _input: bool) {}

    fn play(&mut self, utterance: &str, chunk: usize, _samples: Vec<f32>, _sample_rate: u32) {
        let sink = self.0.lock().unwrap().clone().unwrap();
        sink.send(IoEvent::ChunkStarted {
            utterance: utterance.into(),
            chunk,
        });
        sink.send(IoEvent::ChunkPlayed {
            utterance: utterance.into(),
            chunk,
        });
    }

    fn stop_playback(&mut self) {}

    fn stop(&mut self) {
        *self.0.lock().unwrap() = None;
    }
}

/// The next event `want` keeps, within ten seconds.
async fn next<T>(events: &mut Events, want: impl Fn(VoiceEvent) -> Option<T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), async {
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

/// A loud second, then silence for the turn to end in.
fn turn() -> Vec<f32> {
    let tone = (0..16_000).map(|n| 0.5 * (n as f32 * 0.1).sin());
    tone.chain(std::iter::repeat_n(0.0, 16_000 * 3)).collect()
}

#[test]
fn what_a_call_logs_shows_its_operations_and_holds_no_canary() {
    let captured = Captured::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(captured.clone())
            .finish(),
    )
    .expect("the only subscriber");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async {
        let models = Arc::new(Models(Arc::new(Words(AtomicUsize::new(0)))));
        let sink = Arc::default();
        let input = InputConfig {
            language: Some("en".into()),
            patience: Patience::Fast,
            ..InputConfig::default()
        };
        let (call, mut events) = VoiceCall::new(
            Box::new(Io(Arc::clone(&sink))),
            VoiceSetup {
                input: Some(VoiceInput {
                    models: Arc::clone(&models) as Arc<dyn InputModels>,
                    config: input,
                }),
                output: Some(VoiceOutput {
                    models: models as Arc<dyn OutputModels>,
                    config: OutputConfig::default(),
                }),
                config: VoiceConfig {
                    audio_grace_ms: 0,
                    ..VoiceConfig::default()
                },
            },
        );
        call.start(StartOptions::default());
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == Listening::Listening => Some(()),
            _ => None,
        })
        .await;
        let microphone = sink.lock().unwrap().clone().expect("started");
        for frame in turn().chunks(160) {
            microphone.send(IoEvent::Captured(frame.to_vec()));
        }
        next(&mut events, |event| match event {
            VoiceEvent::Turn(TurnEvent::Finished { .. }) => Some(()),
            _ => None,
        })
        .await;
        for frame in turn().chunks(160) {
            microphone.send(IoEvent::Captured(frame.to_vec()));
        }
        next(&mut events, |event| match event {
            VoiceEvent::Error(_) => Some(()),
            _ => None,
        })
        .await;
        for text in [SAID, SAID_FAILING] {
            let mut saying = call.say(text, SayOptions::default());
            while tokio::time::timeout(Duration::from_secs(10), saying.next())
                .await
                .expect("in time")
                .is_some()
            {}
        }
        call.stop();
        next(&mut events, |event| match event {
            VoiceEvent::State(state) if state.listening == Listening::Idle => Some(()),
            _ => None,
        })
        .await;
    });
    drop(runtime);

    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    // Positive controls: the sites ran, the lines inside the call's task carry its id, and a code that is not plain
    // words is `invalid`.
    for expected in [
        "call started",
        "call stopped",
        "call state",
        "turn started",
        "turn finished",
        "transcribed",
        "transcription failed",
        "synthesized",
        "synthesis failed",
        "voice error",
        "model loaded",
        "model_load{slot=\"vad\"}",
        "call{call=",
        "code=\"invalid\"",
    ] {
        assert!(log.contains(expected), "{expected} missing from:\n{log}");
    }
    // None of the canaries, as text or as the decimal bytes of a `Vec<u8>` rendering.
    for canary in [
        "canary",
        "Canary",
        "7f3a",
        "99, 97, 110, 97, 114, 121",
        "67, 97, 110, 97, 114, 121",
    ] {
        assert!(!log.contains(canary), "{canary} leaked into:\n{log}");
    }
}
