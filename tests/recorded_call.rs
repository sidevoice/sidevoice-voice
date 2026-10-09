//! A whole call on recorded speech, with real models, through the public API only, as an app does: the engine on a
//! `NativeHost` and its bundled catalogue, Silero for voice activity, Whisper base to transcribe and Kokoro to speak.
//! The microphone is a recorded clip (LibriSpeech, `tests/fixtures`), and the speaker hands back what it is given.
//! The person's turn must be heard as the clip's words; the reply is spoken, played and heard to its end, and then
//! heard back through the microphone as a second turn.
//!
//! It downloads about 300 MB the first time (kept by digest in `$SIDEVOICE_TEST_MODELS`, else in
//! `target/test-models`), so it is ignored unless asked for; CI asks, on every native platform:
//!
//! ```sh
//! cargo test --locked --test recorded_call -- --ignored --nocapture
//! ```
// The engine crate, its native host and Tokio are the native build's.
#![cfg(native)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use sidevoice_engine::{BundledCatalog, Engine, NativeHost};
use sidevoice_voice::{
    AudioIo, Events, IoEvent, IoSink, Patience, PlaybackStatus, Reply, RoomEvent, RoomMessage,
    Stage, SttStage, TtsStage, TurnPhase, VoiceCall, VoiceConfig, VoiceEvent,
};

/// The speaker: it keeps what it plays and reports each chunk as played at once.
#[derive(Default)]
struct Speaker {
    sink: Option<IoSink>,
    played: Vec<(Vec<f32>, u32)>,
}

struct RecordedIo(Arc<Mutex<Speaker>>);

impl AudioIo for RecordedIo {
    fn start(&mut self, sink: IoSink) -> Result<(), String> {
        self.0.lock().unwrap().sink = Some(sink);
        Ok(())
    }

    fn play(&mut self, utterance: &str, chunk: usize, samples: Vec<f32>, sample_rate: u32) {
        let mut speaker = self.0.lock().unwrap();
        speaker.played.push((samples, sample_rate));
        let sink = speaker.sink.clone().unwrap();
        for event in [
            IoEvent::ChunkStarted {
                utterance: utterance.into(),
                chunk,
            },
            IoEvent::ChunkPlayed {
                utterance: utterance.into(),
                chunk,
            },
        ] {
            sink.send(event);
        }
    }

    fn stop_playback(&mut self) {}

    fn stop(&mut self) {
        self.0.lock().unwrap().sink = None;
    }
}

/// The samples of a 16-bit mono PCM WAV file.
fn wav(bytes: &[u8]) -> Vec<f32> {
    let at = bytes
        .windows(4)
        .position(|w| w == b"data")
        .expect("a data chunk");
    bytes[at + 8..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| f32::from(i16::from_le_bytes(*pair)) / 32_768.0)
        .collect()
}

/// `samples` at `from` Hz, linearly at 16 kHz.
fn to_16k(samples: &[f32], from: u32) -> Vec<f32> {
    let step = f64::from(from) / 16_000.0;
    (0..(samples.len() as f64 / step) as usize)
        .map(|i| samples[((i as f64 * step) as usize).min(samples.len() - 1)])
        .collect()
}

/// Sends `pcm` as the microphone does, in 10 ms frames, with a second of silence before and three after.
fn speak_into(sink: &IoSink, pcm: &[f32]) {
    let audio = [vec![0.0; 16_000], pcm.to_vec(), vec![0.0; 48_000]].concat();
    for frame in audio.chunks(160) {
        sink.send(IoEvent::Captured(frame.to_vec()));
    }
}

async fn finished_turn(events: &mut Events) -> String {
    loop {
        match events.next().await.expect("the call is alive") {
            VoiceEvent::RoomMessage(RoomMessage::UserTurn(turn))
                if turn.phase == TurnPhase::Finished =>
            {
                return turn.text.expect("a finished turn has its text");
            }
            VoiceEvent::RoomMessage(RoomMessage::UserTurn(turn))
                if turn.phase == TurnPhase::Cancelled =>
            {
                panic!("the turn was cancelled: {turn:?}");
            }
            VoiceEvent::Error(error) => panic!("{}", error.code),
            _ => {}
        }
    }
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

#[test]
#[ignore = "downloads real models (about 300 MB); CI runs it"]
fn a_call_on_recorded_speech_with_real_models() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let dir = std::env::var_os("SIDEVOICE_TEST_MODELS").map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-models"),
            PathBuf::from,
        );
        let host = NativeHost::new(dir).expect("the models' directory");
        let engine =
            Engine::new(Box::new(host), vec![Box::new(BundledCatalog)]).expect("an engine");
        let config = VoiceConfig {
            vad: Stage {
                model: "silero-vad".into(),
                build: Some("silero-vad/sherpa-onnx-fp32".into()),
            },
            stt: SttStage {
                model: "whisper-base".into(),
                build: Some("whisper-base/sherpa-onnx-int8".into()),
                language: Some("en".into()),
            },
            tts: TtsStage {
                model: "kokoro-82m-v1.0".into(),
                build: Some("kokoro-82m-v1.0/sherpa-onnx-int8".into()),
                voice: Some("af_bella".into()),
                speed: 1.0,
            },
            end_of_turn: Default::default(),
            patience: Patience::Fast,
            audio_grace_ms: 0,
            listening_bar: Default::default(),
        };
        let speaker = Arc::new(Mutex::new(Speaker::default()));
        let (call, mut events) = VoiceCall::new(
            Arc::new(engine),
            Box::new(RecordedIo(Arc::clone(&speaker))),
            config,
        );
        call.start();
        let sink = loop {
            if let Some(sink) = speaker.lock().unwrap().sink.clone() {
                break sink;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };

        speak_into(
            &sink,
            &wav(include_bytes!("fixtures/librispeech_mr_quilter.wav")),
        );
        let heard = tokio::time::timeout(Duration::from_secs(300), finished_turn(&mut events))
            .await
            .expect("the turn in time");
        println!("turn: {heard}");
        let heard = words(&heard);
        for word in ["quilter", "apostle", "middle", "classes"] {
            assert!(heard.iter().any(|h| h == word), "{word} in {heard:?}");
        }

        let text = "The weather is lovely today, so we are going for a walk in the park.";
        call.room_event(RoomEvent::Reply(Reply {
            utterance_id: "u1".into(),
            revision: 1,
            reply_revision: 2,
            thread_id: "t".into(),
            history_id: "h".into(),
            text: text.into(),
            language: Some("en-US".into()),
            replay: false,
        }));
        let report = tokio::time::timeout(Duration::from_secs(300), async {
            loop {
                if let VoiceEvent::RoomMessage(RoomMessage::Playback(report)) =
                    events.next().await.unwrap()
                {
                    if report.status != PlaybackStatus::Playing {
                        return report;
                    }
                }
            }
        })
        .await
        .expect("the reply in time");
        assert_eq!(report.status, PlaybackStatus::Heard, "{report:?}");
        assert_eq!(report.heard_chars, text.chars().count());

        let played: Vec<f32> = speaker
            .lock()
            .unwrap()
            .played
            .iter()
            .flat_map(|(samples, rate)| to_16k(samples, *rate))
            .collect();
        assert!(played.len() > 16_000, "Kokoro spoke");
        speak_into(&sink, &played);
        let back = tokio::time::timeout(Duration::from_secs(300), finished_turn(&mut events))
            .await
            .expect("the reply heard back in time");
        println!("heard back: {back}");
        let back = words(&back);
        let said = words(text);
        let found = said.iter().filter(|word| back.contains(word)).count();
        assert!(found * 2 >= said.len(), "half of {said:?} in {back:?}");
        call.stop();
    });
}
