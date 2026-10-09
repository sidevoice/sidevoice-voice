//! The engine crate's models, natively: the call loads each stage with `Engine::load` and runs it through the
//! loaded model's `as_vad`, `as_stt` and `as_tts`. The wasm32 build reaches the engine through the page instead.
#![cfg(native)]

use std::sync::Arc;

use async_trait::async_trait;
use sidevoice_engine::{Cancel, Engine, Error, LoadedModel, Progress, VadOptions, VadStream};

use super::{
    Detector, Loaded, Models, Speaker, Transcriber, VAD_MIN_SILENCE_MS, VAD_MIN_SPEECH_MS,
    VAD_THRESHOLD,
};
use crate::config::VoiceConfig;
use crate::turns::RATE;

/// The models of an engine.
pub(crate) struct EngineModels(pub(crate) Arc<Engine>);

#[async_trait]
impl Models for EngineModels {
    async fn load(&self, config: &VoiceConfig) -> Result<Loaded, String> {
        let vad = self
            .model(&config.vad.model, config.vad.build.as_deref())
            .await?;
        let stt = self
            .model(&config.stt.model, config.stt.build.as_deref())
            .await?;
        let tts = self
            .model(&config.tts.model, config.tts.build.as_deref())
            .await?;
        let options = VadOptions {
            threshold: VAD_THRESHOLD,
            min_silence_ms: VAD_MIN_SILENCE_MS,
            min_speech_ms: VAD_MIN_SPEECH_MS,
        };
        let stream = vad
            .as_vad()
            .ok_or("model-cannot-detect")?
            .stream(options)
            .await
            .map_err(code)?;
        if stream.sample_rate() != RATE {
            return Err("vad-rate-unsupported".into());
        }
        stt.as_stt().ok_or("model-cannot-transcribe")?;
        let voice = match &config.tts.voice {
            Some(voice) => voice.clone(),
            None => {
                let tts = tts.as_tts().ok_or("model-cannot-speak")?;
                let voices = tts.voices().await;
                voices.first().ok_or("model-has-no-voice")?.id.clone()
            }
        };
        Ok(Loaded {
            detector: Box::new(EngineDetector(stream)),
            transcriber: Arc::new(EngineTranscriber(stt)),
            speaker: Arc::new(EngineSpeaker {
                model: tts,
                voice,
                speed: config.tts.speed,
            }),
        })
    }
}

impl EngineModels {
    async fn model(&self, model: &str, build: Option<&str>) -> Result<LoadedModel, String> {
        self.0
            .load(model, build, &|_: Progress| {}, &Cancel::new())
            .await
            .map_err(code)
    }
}

struct EngineDetector(VadStream);

#[async_trait]
impl Detector for EngineDetector {
    async fn accept(&mut self, pcm: &[f32]) -> Result<Vec<(u64, bool)>, String> {
        let output = self.0.accept(pcm).await.map_err(code)?;
        Ok(output
            .frames
            .iter()
            .map(|frame| (frame.end, frame.speech))
            .collect())
    }

    async fn reset(&mut self) {
        self.0.reset();
    }
}

struct EngineTranscriber(LoadedModel);

#[async_trait]
impl Transcriber for EngineTranscriber {
    async fn transcribe(&self, pcm: Vec<f32>, language: Option<String>) -> Result<String, String> {
        let stt = self.0.as_stt().ok_or("model-cannot-transcribe")?;
        stt.transcribe(&pcm, RATE, language.as_deref())
            .await
            .map_err(code)
    }
}

struct EngineSpeaker {
    model: LoadedModel,
    voice: String,
    speed: f32,
}

#[async_trait]
impl Speaker for EngineSpeaker {
    async fn speak(
        &self,
        text: String,
        language: Option<String>,
    ) -> Result<(Vec<f32>, u32), String> {
        let tts = self.model.as_tts().ok_or("model-cannot-speak")?;
        let audio = tts
            .speak(&text, &self.voice, language.as_deref(), Some(self.speed))
            .await
            .map_err(code)?;
        Ok((audio.samples, audio.sample_rate))
    }
}

fn code(error: Error) -> String {
    error.code.to_owned()
}
