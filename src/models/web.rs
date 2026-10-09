//! The engine's models in the wasm32 build: the page's `WebEngine` (the npm package `@sidevoice/engine`), reached
//! through its JavaScript methods, since two WebAssembly modules share no Rust objects. The call loads each stage with
//! `engine.load`, and runs `asVad().stream`, `asStt().transcribe` and `asTts().speak` on what it returns. A rejection
//! carries the engine's stable `code`.
#![cfg(web)]

use std::sync::Arc;

use async_trait::async_trait;
use js_sys::{Array, Float32Array, Function, Object, Promise, Reflect};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use super::{
    Detector, Loaded, Models, Speaker, Transcriber, VAD_MIN_SILENCE_MS, VAD_MIN_SPEECH_MS,
    VAD_THRESHOLD,
};
use crate::config::VoiceConfig;
use crate::turns::RATE;

/// The page's `WebEngine`.
pub(crate) struct WebModels(pub(crate) JsValue);

#[async_trait(?Send)]
impl Models for WebModels {
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
        let vad = capability(&vad, "asVad", "model-cannot-detect")?;
        let options = Object::new();
        set(&options, "threshold", &VAD_THRESHOLD.into());
        set(&options, "minSilenceMs", &VAD_MIN_SILENCE_MS.into());
        set(&options, "minSpeechMs", &VAD_MIN_SPEECH_MS.into());
        let stream = call(&vad, "stream", &[options.into()]).await?;
        if Reflect::get(&stream, &"sampleRate".into())
            .ok()
            .and_then(|rate| rate.as_f64())
            != Some(f64::from(RATE))
        {
            return Err("vad-rate-unsupported".into());
        }
        let stt = capability(&stt, "asStt", "model-cannot-transcribe")?;
        let tts = capability(&tts, "asTts", "model-cannot-speak")?;
        let voice = match &config.tts.voice {
            Some(voice) => voice.clone(),
            None => {
                let voices: Array = call(&tts, "voices", &[]).await?.unchecked_into();
                let first = voices.get(0);
                Reflect::get(&first, &"id".into())
                    .ok()
                    .and_then(|id| id.as_string())
                    .ok_or("model-has-no-voice")?
            }
        };
        Ok(Loaded {
            detector: Box::new(WebDetector(stream)),
            transcriber: Arc::new(WebTranscriber(stt)),
            speaker: Arc::new(WebSpeaker {
                tts,
                voice,
                speed: config.tts.speed,
            }),
        })
    }
}

impl WebModels {
    async fn model(&self, model: &str, build: Option<&str>) -> Result<JsValue, String> {
        let build = build.map_or(JsValue::UNDEFINED, JsValue::from);
        call(&self.0, "load", &[model.into(), build]).await
    }
}

struct WebDetector(JsValue);

#[async_trait(?Send)]
impl Detector for WebDetector {
    async fn accept(&mut self, pcm: &[f32]) -> Result<Vec<(u64, bool)>, String> {
        let output = call(&self.0, "accept", &[Float32Array::from(pcm).into()]).await?;
        let frames: Array = Reflect::get(&output, &"frames".into())
            .map_err(|_| "detection-failed")?
            .unchecked_into();
        Ok(frames
            .iter()
            .map(|frame| {
                let end = Reflect::get(&frame, &"end".into())
                    .ok()
                    .and_then(|end| end.as_f64());
                let speech = Reflect::get(&frame, &"speech".into())
                    .ok()
                    .and_then(|speech| speech.as_bool());
                (end.unwrap_or_default() as u64, speech.unwrap_or_default())
            })
            .collect())
    }

    async fn reset(&mut self) {
        let _ = call(&self.0, "reset", &[]).await;
    }
}

struct WebTranscriber(JsValue);

#[async_trait(?Send)]
impl Transcriber for WebTranscriber {
    async fn transcribe(&self, pcm: Vec<f32>, language: Option<String>) -> Result<String, String> {
        let language = language.map_or(JsValue::UNDEFINED, JsValue::from);
        let text = call(
            &self.0,
            "transcribe",
            &[Float32Array::from(&pcm[..]).into(), RATE.into(), language],
        )
        .await?;
        text.as_string()
            .ok_or_else(|| "transcription-failed".into())
    }
}

struct WebSpeaker {
    tts: JsValue,
    voice: String,
    speed: f32,
}

#[async_trait(?Send)]
impl Speaker for WebSpeaker {
    async fn speak(
        &self,
        text: String,
        language: Option<String>,
    ) -> Result<(Vec<f32>, u32), String> {
        let language = language.map_or(JsValue::UNDEFINED, JsValue::from);
        let args = [
            text.into(),
            self.voice.clone().into(),
            language,
            self.speed.into(),
        ];
        let audio = call(&self.tts, "speak", &args).await?;
        let samples = Reflect::get(&audio, &"samples".into())
            .ok()
            .and_then(|samples| samples.dyn_into::<Float32Array>().ok())
            .ok_or("synthesis-failed")?;
        let rate = Reflect::get(&audio, &"sampleRate".into())
            .ok()
            .and_then(|rate| rate.as_f64())
            .ok_or("synthesis-failed")?;
        Ok((samples.to_vec(), rate as u32))
    }
}

/// `model.<method>()`, the loaded model as one capability, or `missing` when it has not that capability.
fn capability(model: &JsValue, method: &str, missing: &str) -> Result<JsValue, String> {
    let function = function(model, method)?;
    let value = function.call0(model).map_err(code)?;
    if value.is_undefined() || value.is_null() {
        return Err(missing.to_owned());
    }
    Ok(value)
}

/// `object.<method>(...args)`, awaited when it returns a promise.
async fn call(object: &JsValue, method: &str, args: &[JsValue]) -> Result<JsValue, String> {
    let function = function(object, method)?;
    let args: Array = args.iter().collect();
    let value = function.apply(object, &args).map_err(code)?;
    match value.dyn_into::<Promise>() {
        Ok(promise) => JsFuture::from(promise).await.map_err(code),
        Err(value) => Ok(value),
    }
}

fn function(object: &JsValue, method: &str) -> Result<Function, String> {
    Reflect::get(object, &method.into())
        .ok()
        .and_then(|function| function.dyn_into::<Function>().ok())
        .ok_or_else(|| format!("engine-method-missing-{method}"))
}

/// The engine's stable code of a rejection, `engine-failed` when it carries none.
fn code(error: JsValue) -> String {
    Reflect::get(&error, &"code".into())
        .ok()
        .and_then(|code| code.as_string())
        .unwrap_or_else(|| "engine-failed".into())
}

fn set(object: &Object, key: &str, value: &JsValue) {
    let _ = Reflect::set(object, &key.into(), value);
}
