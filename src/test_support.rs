//! Test doubles shared by the tests of every module: the recorded clips (`tests/fixtures`, compiled in so the wasm32
//! tests read them too), a voice activity detector on energy that answers as the engine's streams do, and a
//! configuration.

use crate::config::{Stage, SttStage, TtsStage, VoiceConfig};
use crate::turns::instant;

/// LibriSpeech dev-clean 1272-128104-0000, English, 16 kHz mono (tests/fixtures/README.md).
pub(crate) const QUILTER: &[u8] = include_bytes!("../tests/fixtures/librispeech_mr_quilter.wav");
/// FLEURS es_419, Spanish, 16 kHz mono (tests/fixtures/README.md).
pub(crate) const FLEURS_ES: &[u8] = include_bytes!("../tests/fixtures/fleur_es_sample.wav");

/// Samples in one detector window: Silero's at 16 kHz.
pub(crate) const WINDOW: usize = 512;

/// The samples of a 16-bit mono PCM WAV file, from −1 to 1, scaled so that the loudest is at `peak`.
pub(crate) fn clip(wav: &[u8], peak: f32) -> Vec<f32> {
    assert_eq!(&wav[..4], b"RIFF", "a WAV file");
    let mut at = 12;
    while &wav[at..at + 4] != b"data" {
        let size = u32::from_le_bytes(wav[at + 4..at + 8].try_into().expect("a chunk size"));
        at += 8 + size as usize;
    }
    let size = u32::from_le_bytes(wav[at + 4..at + 8].try_into().expect("a chunk size")) as usize;
    let samples: Vec<f32> = wav[at + 8..at + 8 + size]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| f32::from(i16::from_le_bytes(*pair)) / 32_768.0)
        .collect();
    let loudest = samples.iter().fold(0.0_f32, |max, s| max.max(s.abs()));
    samples.iter().map(|s| s * peak / loudest).collect()
}

/// `ms` milliseconds of 16 kHz silence.
pub(crate) fn silence(ms: usize) -> Vec<f32> {
    vec![0.0; ms * 16]
}

/// A voice activity detector on energy, with the engine's stream semantics: speech is confirmed after
/// `min_speech_ms` above the threshold, and ends after `min_silence_ms` below it.
pub(crate) struct EnergyVad {
    above: usize,
    below: usize,
    speech: bool,
}

impl EnergyVad {
    const THRESHOLD: f32 = 0.45;
    const MIN_SPEECH: usize = 400 * 16;
    const MIN_SILENCE: usize = 200 * 16;

    pub(crate) fn new() -> Self {
        Self {
            above: 0,
            below: 0,
            speech: false,
        }
    }

    /// Whether the stream is inside speech after this window.
    pub(crate) fn window(&mut self, pcm: &[f32]) -> bool {
        if instant(pcm) >= Self::THRESHOLD {
            self.above += pcm.len();
            self.below = 0;
            if self.above >= Self::MIN_SPEECH {
                self.speech = true;
            }
        } else {
            self.below += pcm.len();
            if self.below >= Self::MIN_SILENCE {
                self.speech = false;
                self.above = 0;
            }
        }
        self.speech
    }
}

/// A configuration with the default numbers.
pub(crate) fn config() -> VoiceConfig {
    VoiceConfig {
        vad: Stage {
            model: "silero-vad".into(),
            build: None,
        },
        stt: SttStage {
            model: "whisper-base".into(),
            build: None,
            language: Some("en".into()),
        },
        tts: TtsStage {
            model: "kokoro-82m-v1.0".into(),
            build: None,
            voice: None,
            speed: 1.0,
        },
        end_of_turn: Default::default(),
        patience: Default::default(),
        audio_grace_ms: 1_000,
        listening_bar: Default::default(),
    }
}
