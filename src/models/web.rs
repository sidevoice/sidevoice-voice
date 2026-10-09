//! The app's models in the wasm32 build: JavaScript objects with the interfaces' methods, reached through
//! wasm-bindgen's structural imports (`js/voice-models.d.ts` types them). A method may answer a value or a promise of
//! it; a rejection's `code` is the failure's, else the method's own failure code.
#![cfg(web)]

use std::sync::Arc;

use async_trait::async_trait;
use js_sys::{Array, Float32Array, Promise, Reflect};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use super::{EndOfTurnModel, Models, Speaker, Transcriber, Vad, VadFrame, VoiceModels};

#[wasm_bindgen]
extern "C" {
    /// `VoiceModels`: `{ load(): Promise<{vad, transcriber, speaker, endOfTurn?}> }`.
    pub type JsVoiceModels;
    #[wasm_bindgen(method, catch)]
    fn load(this: &JsVoiceModels) -> Result<JsValue, JsValue>;

    type JsVad;
    #[wasm_bindgen(method, catch)]
    fn accept(this: &JsVad, samples: Float32Array) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(method, catch)]
    fn reset(this: &JsVad) -> Result<JsValue, JsValue>;

    type JsTranscriber;
    #[wasm_bindgen(method, catch)]
    fn transcribe(
        this: &JsTranscriber,
        samples: Float32Array,
        sample_rate: u32,
        language: Option<String>,
    ) -> Result<JsValue, JsValue>;

    type JsSpeaker;
    #[wasm_bindgen(method, catch)]
    fn speak(
        this: &JsSpeaker,
        text: String,
        voice: Option<String>,
        language: Option<String>,
        speed: f32,
    ) -> Result<JsValue, JsValue>;

    type JsEndOfTurn;
    #[wasm_bindgen(method, catch, js_name = endOfTurn)]
    fn end_of_turn(
        this: &JsEndOfTurn,
        samples: Float32Array,
        sample_rate: u32,
    ) -> Result<JsValue, JsValue>;
}

#[async_trait(?Send)]
impl VoiceModels for JsVoiceModels {
    async fn load(&self) -> Result<Models, String> {
        let loaded = settle(JsVoiceModels::load(self), "models-load-failed").await?;
        let field = |name: &str| Reflect::get(&loaded, &name.into()).unwrap_or(JsValue::UNDEFINED);
        let present = |value: &JsValue| !value.is_undefined() && !value.is_null();
        let (vad, transcriber, speaker) = (field("vad"), field("transcriber"), field("speaker"));
        if !present(&vad) || !present(&transcriber) || !present(&speaker) {
            return Err("models-incomplete".into());
        }
        let end_of_turn = field("endOfTurn");
        Ok(Models {
            vad: Box::new(WebVad(vad.unchecked_into())),
            transcriber: Arc::new(WebTranscriber(transcriber.unchecked_into())),
            speaker: Arc::new(WebSpeaker(speaker.unchecked_into())),
            end_of_turn: present(&end_of_turn).then(|| {
                Arc::new(WebEndOfTurn(end_of_turn.unchecked_into())) as Arc<dyn EndOfTurnModel>
            }),
        })
    }
}

struct WebVad(JsVad);

#[async_trait(?Send)]
impl Vad for WebVad {
    async fn accept(&mut self, pcm: &[f32]) -> Result<Vec<VadFrame>, String> {
        let frames = settle(self.0.accept(Float32Array::from(pcm)), "vad-failed").await?;
        let frames: Array = frames.dyn_into().map_err(|_| "vad-failed".to_owned())?;
        Ok(frames
            .iter()
            .map(|frame| {
                let number = |name: &str| {
                    Reflect::get(&frame, &name.into())
                        .ok()
                        .and_then(|v| v.as_f64())
                };
                VadFrame {
                    end: number("end").unwrap_or_default() as u64,
                    speech: Reflect::get(&frame, &"speech".into())
                        .ok()
                        .and_then(|v| v.as_bool())
                        .unwrap_or_default(),
                    probability: number("probability").map(|p| p as f32),
                }
            })
            .collect())
    }

    async fn reset(&mut self) {
        let _ = settle(self.0.reset(), "vad-failed").await;
    }
}

struct WebTranscriber(JsTranscriber);

#[async_trait(?Send)]
impl Transcriber for WebTranscriber {
    async fn transcribe(
        &self,
        pcm: Vec<f32>,
        sample_rate: u32,
        language: Option<String>,
    ) -> Result<String, String> {
        let text = self
            .0
            .transcribe(Float32Array::from(&pcm[..]), sample_rate, language);
        let text = settle(text, "transcription-failed").await?;
        text.as_string()
            .ok_or_else(|| "transcription-failed".into())
    }
}

struct WebSpeaker(JsSpeaker);

#[async_trait(?Send)]
impl Speaker for WebSpeaker {
    async fn speak(
        &self,
        text: String,
        voice: Option<String>,
        language: Option<String>,
        speed: f32,
    ) -> Result<(Vec<f32>, u32), String> {
        let audio = settle(
            self.0.speak(text, voice, language, speed),
            "synthesis-failed",
        )
        .await?;
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

struct WebEndOfTurn(JsEndOfTurn);

#[async_trait(?Send)]
impl EndOfTurnModel for WebEndOfTurn {
    async fn end_of_turn(&self, pcm: Vec<f32>, sample_rate: u32) -> Result<f32, String> {
        let p = self
            .0
            .end_of_turn(Float32Array::from(&pcm[..]), sample_rate);
        let p = settle(p, "end-of-turn-failed").await?;
        p.as_f64()
            .map(|p| p as f32)
            .ok_or_else(|| "end-of-turn-failed".into())
    }
}

/// What a method answered, awaited if it is a promise; a throw or a rejection as its `code`, else `failure`.
async fn settle(answer: Result<JsValue, JsValue>, failure: &str) -> Result<JsValue, String> {
    let code = |error: JsValue| {
        Reflect::get(&error, &"code".into())
            .ok()
            .and_then(|code| code.as_string())
            .unwrap_or_else(|| failure.to_owned())
    };
    let value = answer.map_err(code)?;
    JsFuture::from(Promise::resolve(&value)).await.map_err(code)
}
