//! The bridge to JavaScript, only in the wasm32 build (the npm package): `VoiceCall.create(models, io, config)`, the
//! same call as the Rust [`VoiceCall`](crate::VoiceCall), on the page's models and a JavaScript microphone and speaker.
//!
//! - `models` is the page's `VoiceModels` (`js/voice-models.d.ts`): `load()` answers `{vad, transcriber, speaker,
//!   endOfTurn?}`, objects with the methods of [`Vad`](crate::Vad), [`Transcriber`](crate::Transcriber),
//!   [`Speaker`](crate::Speaker) and [`EndOfTurnModel`](crate::EndOfTurnModel).
//! - `io` is an object with `start(sink)`, `play(utterance, chunk, samples, sampleRate)`, `stopPlayback()` and
//!   `stop()`; it reports through `sink` (an [`IoSink`](crate::IoSink): `ready()` once the microphone and the
//!   speaker work, which is when the call listens, `captured(samples)`, with 16 kHz mono samples,
//!   `chunkStarted(utterance, chunk)`, `chunkPlayed(utterance, chunk)`, `failed(code)`). `start` may throw an error
//!   with a `code`.
//! - `config` is the configuration as JSON ([`VoiceConfig`]); a malformed one throws.
//!
//! Every event reaches the callbacks given to `onEvent`, as `{ type, data }`, the JSON of
//! [`VoiceEvent`](crate::VoiceEvent). `say(text, options)` answers a `Saying` handle.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use futures_util::StreamExt;
use js_sys::{Array, Float32Array, Function, Promise, Reflect, JSON};
use wasm_bindgen::prelude::*;

use crate::io::{AudioIo, IoEvent, IoSink};
use crate::models::JsVoiceModels;
use crate::runtime::spawn;
use crate::say::{SayCancel, SayEvent, SayOptions, Saying};
use crate::voice_call::VoiceCall;
use crate::VoiceConfig;

/// One voice call, for JavaScript.
#[wasm_bindgen(js_name = VoiceCall)]
pub struct WebVoiceCall {
    call: VoiceCall,
    listeners: Rc<RefCell<Vec<Function>>>,
}

#[wasm_bindgen(js_class = VoiceCall)]
impl WebVoiceCall {
    /// A call on the page's `models` and `io`'s microphone and speaker, set up with `config`. It does nothing until
    /// `start()`.
    pub fn create(models: JsValue, io: JsValue, config: JsValue) -> Result<WebVoiceCall, JsError> {
        let config = read_config(&config)?;
        let (call, mut events) = VoiceCall::new(
            Arc::new(models.unchecked_into::<JsVoiceModels>()),
            Box::new(JsIo(io)),
            config,
        );
        let listeners: Rc<RefCell<Vec<Function>>> = Rc::default();
        let heard = Rc::clone(&listeners);
        spawn(async move {
            while let Some(event) = events.next().await {
                let json = serde_json::to_string(&event).expect("events serialize");
                let value = JSON::parse(&json).expect("JSON");
                for listener in heard.borrow().iter() {
                    let _ = listener.call1(&JsValue::NULL, &value);
                }
            }
        });
        Ok(Self { call, listeners })
    }

    /// Calls `listener` with every event from now on.
    #[wasm_bindgen(js_name = onEvent)]
    pub fn on_event(&self, listener: Function) {
        self.listeners.borrow_mut().push(listener);
    }

    /// Loads the models, opens the microphone and the speaker, and starts listening.
    pub fn start(&self) {
        self.call.start();
    }

    /// Stops listening and speaking; the models stay loaded.
    pub fn stop(&self) {
        self.call.stop();
    }

    /// A new configuration, as JSON.
    #[wasm_bindgen(js_name = setConfig)]
    pub fn set_config(&self, config: JsValue) -> Result<(), JsError> {
        self.call.set_config(read_config(&config)?);
        Ok(())
    }

    /// Other models (`VoiceModels`) with the configuration they go with, as JSON, taken together: loaded at once if the
    /// call is started, else at the next start. A malformed configuration throws and changes nothing.
    #[wasm_bindgen(js_name = setModels)]
    pub fn set_models(&self, models: JsValue, config: JsValue) -> Result<(), JsError> {
        let config = read_config(&config)?;
        self.call
            .set_models(Arc::new(models.unchecked_into::<JsVoiceModels>()), config);
        Ok(())
    }

    /// Says `text` (`options`: `{ language? }`) after whatever is being said, and answers its handle.
    pub fn say(&self, text: String, options: JsValue) -> Result<WebSaying, JsError> {
        let language = if options.is_undefined() || options.is_null() {
            None
        } else {
            Reflect::get(&options, &"language".into())
                .ok()
                .and_then(|language| language.as_string())
        };
        let saying = self.call.say(text, SayOptions { language });
        Ok(WebSaying::new(saying))
    }

    /// Mutes or unmutes the microphone.
    pub fn mute(&self, muted: bool) {
        self.call.mute(muted);
    }

    /// Cancels what the person said that is not reported yet.
    #[wasm_bindgen(js_name = cancelInput)]
    pub fn cancel_input(&self) {
        self.call.cancel_input();
    }
}

/// Something the call is saying, for JavaScript: its `id`, `cancel()`, `onEvent(listener)` for its steps in order
/// (`{ type: "playing" }`, `{ type: "progress", sounding, heard_chars }`, `{ type: "done", outcome }`), and `outcome`, a
/// promise of how it ended (`{ status: "heard" }`, `{ status: "heard-up-to", heard_chars, reason }`,
/// `{ status: "not-played", reason }`).
#[wasm_bindgen(js_name = Saying)]
pub struct WebSaying {
    id: String,
    cancel: SayCancel,
    listeners: Rc<RefCell<Vec<Function>>>,
    outcome: Promise,
}

impl WebSaying {
    fn new(mut saying: Saying) -> Self {
        let mut resolve = None;
        let outcome = Promise::new(&mut |resolved, _| resolve = Some(resolved));
        let resolve = resolve.expect("the promise's resolve");
        let listeners: Rc<RefCell<Vec<Function>>> = Rc::default();
        let heard = Rc::clone(&listeners);
        let (id, cancel) = (saying.id().to_owned(), saying.canceller());
        spawn(async move {
            while let Some(event) = saying.next().await {
                let json = serde_json::to_string(&event).expect("events serialize");
                let value = JSON::parse(&json).expect("JSON");
                for listener in heard.borrow().iter() {
                    let _ = listener.call1(&JsValue::NULL, &value);
                }
                if let SayEvent::Done { outcome } = event {
                    let json = serde_json::to_string(&outcome).expect("outcomes serialize");
                    let _ = resolve.call1(&JsValue::NULL, &JSON::parse(&json).expect("JSON"));
                }
            }
        });
        Self {
            id,
            cancel,
            listeners,
            outcome,
        }
    }
}

#[wasm_bindgen(js_class = Saying)]
impl WebSaying {
    /// The call's id for it.
    #[wasm_bindgen(getter)]
    pub fn id(&self) -> String {
        self.id.clone()
    }

    /// Cancels it: the part not yet heard is dropped, and its outcome says `cancelled`.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Calls `listener` with each of its steps from now on.
    #[wasm_bindgen(js_name = onEvent)]
    pub fn on_event(&self, listener: Function) {
        self.listeners.borrow_mut().push(listener);
    }

    /// How it ended, once it has.
    #[wasm_bindgen(getter)]
    pub fn outcome(&self) -> Promise {
        self.outcome.clone()
    }
}

/// Where a JavaScript microphone and speaker report, for JavaScript.
#[wasm_bindgen(js_class = IoSink)]
impl IoSink {
    /// The microphone and the speaker work: the call listens from now.
    pub fn ready(&self) {
        self.send(IoEvent::Ready);
    }

    /// Captured audio: 16 kHz mono samples, in order, after echo cancellation.
    pub fn captured(&self, samples: Vec<f32>) {
        self.send(IoEvent::Captured(samples));
    }

    /// The first sample of a chunk reached the speaker.
    #[wasm_bindgen(js_name = chunkStarted)]
    pub fn chunk_started(&self, utterance: String, chunk: usize) {
        self.send(IoEvent::ChunkStarted { utterance, chunk });
    }

    /// The last sample of a chunk reached the speaker.
    #[wasm_bindgen(js_name = chunkPlayed)]
    pub fn chunk_played(&self, utterance: String, chunk: usize) {
        self.send(IoEvent::ChunkPlayed { utterance, chunk });
    }

    /// Whether the microphone's device gives no audio for now (muted by the browser or the system, not by the
    /// person), each time that changes.
    #[wasm_bindgen(js_name = microphoneMuted)]
    pub fn microphone_muted(&self, muted: bool) {
        self.send(IoEvent::MicrophoneMuted(muted));
    }

    /// The microphone or the speaker failed, with a stable code; the call stops.
    pub fn failed(&self, code: String) {
        self.send(IoEvent::Failed(code));
    }
}

/// A JavaScript microphone and speaker.
struct JsIo(JsValue);

impl AudioIo for JsIo {
    fn start(&mut self, sink: IoSink) -> Result<(), String> {
        self.call("start", &[sink.into()])
    }

    fn play(&mut self, utterance: &str, chunk: usize, samples: Vec<f32>, sample_rate: u32) {
        let samples = Float32Array::from(&samples[..]);
        let _ = self.call(
            "play",
            &[
                utterance.into(),
                chunk.into(),
                samples.into(),
                sample_rate.into(),
            ],
        );
    }

    fn stop_playback(&mut self) {
        let _ = self.call("stopPlayback", &[]);
    }

    fn stop(&mut self) {
        let _ = self.call("stop", &[]);
    }
}

impl JsIo {
    fn call(&self, method: &str, args: &[JsValue]) -> Result<(), String> {
        let function = Reflect::get(&self.0, &method.into())
            .ok()
            .and_then(|function| function.dyn_into::<Function>().ok())
            .ok_or_else(|| format!("io-method-missing-{method}"))?;
        let args: Array = args.iter().collect();
        function.apply(&self.0, &args).map(|_| ()).map_err(|error| {
            Reflect::get(&error, &"code".into())
                .ok()
                .and_then(|code| code.as_string())
                .unwrap_or_else(|| "io-failed".into())
        })
    }
}

fn read_config(config: &JsValue) -> Result<VoiceConfig, JsError> {
    serde_json::from_value(json(config)?).map_err(|error| JsError::new(&error.to_string()))
}

fn json(value: &JsValue) -> Result<serde_json::Value, JsError> {
    let text = JSON::stringify(value)
        .map_err(|_| JsError::new("not JSON"))?
        .as_string()
        .ok_or_else(|| JsError::new("not JSON"))?;
    serde_json::from_str(&text).map_err(|error| JsError::new(&error.to_string()))
}
