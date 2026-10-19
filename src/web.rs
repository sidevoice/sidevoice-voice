//! The bridge to JavaScript, only in the wasm32 build (the npm package): `VoiceCall.create(io, setup)`, the same call as
//! the Rust [`VoiceCall`](crate::VoiceCall), on the page's models and a JavaScript microphone and speaker.
//!
//! - `setup` is the call's [`VoiceSetup`]: `{input?: {models, ...InputConfig}, output?: {models, ...OutputConfig},
//!   ...VoiceConfig}`. A half that is absent (or null) is not there. Each half's `models` is the page's object
//!   (`js/voice-models.d.ts`): the input's `{vad(), transcriber(), endOfTurn?()}`, the output's `{speaker()}`, each
//!   answering the loaded model, an object with the methods of [`Vad`](crate::Vad), [`Transcriber`](crate::Transcriber),
//!   [`EndOfTurnModel`](crate::EndOfTurnModel) or [`Speaker`](crate::Speaker). The rest is the configuration, as JSON
//!   read strictly; a malformed one throws. The same `models` object in a later `update` is the same models.
//! - `io` is an object with `start(sink, {input})`, `setInput(input)`, `play(utterance, chunk, samples, sampleRate)`,
//!   `stopPlayback()` and `stop()`; it reports through `sink` (an [`IoSink`](crate::IoSink): `ready()` once what was
//!   asked for works, which is when the call listens, `captured(samples)`, with 16 kHz mono samples,
//!   `rendered(samples)` with 16 kHz post-gain speaker samples, `playbackEnded()` as the terminal playback marker,
//!   `chunkStarted(utterance, chunk)`, `chunkPlayed(utterance, chunk)`, `inputLost(code)`, `failed(code)`). `start`
//!   may throw an error with a `code`.
//!
//! Every event reaches the callbacks given to `onEvent`, as `{ type, data }`, the JSON of
//! [`VoiceEvent`](crate::VoiceEvent), but a take's samples, which are a `Float32Array` (`{type: "take", data: {sampleRate,
//! samples, started_at, ended_at, turns}}`). `start({take})` asks for one. `say(text, options)` answers a `Saying` handle.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use futures_util::StreamExt;
use js_sys::{Array, Float32Array, Function, Object, Promise, Reflect, JSON};
use wasm_bindgen::prelude::*;

use crate::config::{InputConfig, OutputConfig, VoiceConfig};
use crate::io::{AudioIo, IoEvent, IoSink};
use crate::models::{InputModels, JsInputModels, JsOutputModels, OutputModels};
use crate::runtime::spawn;
use crate::say::{SayCancel, SayEvent, SayOptions, Saying};
use crate::setup::{VoiceInput, VoiceOutput, VoiceSetup};
use crate::take::Take;
use crate::voice_call::{StartOptions, VoiceCall};
use crate::VoiceEvent;

/// The Rust event keeps `StopReason` as a nested field. The page's `VoiceSayEvent` and
/// `VoiceSayOutcome` put its `reason` (and failure `code`) beside `status`.
fn host_say_event_json(event: &SayEvent) -> String {
    let mut value = serde_json::to_value(event).expect("say events serialize");
    if let SayEvent::Done { .. } = event {
        let outcome = value["outcome"]
            .as_object_mut()
            .expect("say outcome object");
        if let Some(reason) = outcome.remove("reason") {
            outcome.extend(reason.as_object().expect("stop reason object").clone());
        }
    }
    serde_json::to_string(&value).expect("say events serialize")
}

/// One voice call, for JavaScript.
#[wasm_bindgen(js_name = VoiceCall)]
pub struct WebVoiceCall {
    call: VoiceCall,
    listeners: Rc<RefCell<Vec<Function>>>,
    /// The page's model objects of the last setup, and the models made of them: the same object is the same models.
    kept: RefCell<Kept>,
}

/// The page's model objects each half was last given, with what the call was given for them.
#[derive(Default)]
struct Kept {
    input: Option<(JsValue, Arc<dyn InputModels>)>,
    output: Option<(JsValue, Arc<dyn OutputModels>)>,
}

#[wasm_bindgen(js_class = VoiceCall)]
impl WebVoiceCall {
    /// A call with the halves of `setup`, on `io`'s microphone and speaker. It loads and opens nothing until
    /// `start()`.
    pub fn create(io: JsValue, setup: JsValue) -> Result<WebVoiceCall, JsError> {
        let mut kept = Kept::default();
        let setup = read_setup(&setup, &mut kept)?;
        let (call, mut events) = VoiceCall::new(Box::new(JsIo(io)), setup);
        let listeners: Rc<RefCell<Vec<Function>>> = Rc::default();
        let heard = Rc::clone(&listeners);
        spawn(async move {
            while let Some(event) = events.next().await {
                let value = match event {
                    VoiceEvent::Take(take) => take_event(take),
                    event => {
                        let json = serde_json::to_string(&event).expect("events serialize");
                        JSON::parse(&json).expect("JSON")
                    }
                };
                for listener in heard.borrow().iter() {
                    let _ = listener.call1(&JsValue::NULL, &value);
                }
            }
        });
        Ok(Self {
            call,
            listeners,
            kept: RefCell::new(kept),
        })
    }

    /// Calls `listener` with every event from now on.
    #[wasm_bindgen(js_name = onEvent)]
    pub fn on_event(&self, listener: Function) {
        self.listeners.borrow_mut().push(listener);
    }

    /// Loads the models, opens the microphone and the speaker, and starts listening. `options`: `{ take? }`, `true` to
    /// keep the take, told as a `take` event once the listening ends.
    pub fn start(&self, options: JsValue) {
        let take = !options.is_undefined()
            && !options.is_null()
            && Reflect::get(&options, &"take".into())
                .ok()
                .and_then(|take| take.as_bool())
                .unwrap_or(false);
        self.call.start(StartOptions { take });
    }

    /// Stops listening and speaking; the models stay loaded.
    pub fn stop(&self) {
        self.call.stop();
    }

    /// Another setup, the whole of it ([`VoiceCall::update`]). A malformed one throws and changes nothing.
    pub fn update(&self, setup: JsValue) -> Result<(), JsError> {
        let mut kept = self.kept.borrow_mut();
        let mut next = Kept {
            input: kept.input.clone(),
            output: kept.output.clone(),
        };
        let setup = read_setup(&setup, &mut next)?;
        *kept = next;
        self.call.update(setup);
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

    /// Closes the open turn now with what was captured so far, to be told as `finished` like any turn; nothing happens
    /// when none is open. Resolves once the call has closed it.
    #[wasm_bindgen(js_name = endInput)]
    pub fn end_input(&self) -> Promise {
        let closed = self.call.end_input();
        wasm_bindgen_futures::future_to_promise(async move {
            closed.await;
            Ok(JsValue::UNDEFINED)
        })
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
                let json = host_say_event_json(&event);
                let value = JSON::parse(&json).expect("JSON");
                for listener in heard.borrow().iter() {
                    let _ = listener.call1(&JsValue::NULL, &value);
                }
                if let SayEvent::Done { .. } = event {
                    let outcome = Reflect::get(&value, &"outcome".into()).expect("done outcome");
                    let _ = resolve.call1(&JsValue::NULL, &outcome);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::say::{SayOutcome, StopReason};
    use futures_channel::mpsc::unbounded;
    use wasm_bindgen_futures::JsFuture;
    use wasm_bindgen_test::wasm_bindgen_test as test;

    /// `value`'s JSON as a JavaScript object, with `models` set to `models` in each half named.
    fn setup_of(value: serde_json::Value, halves: &[(&str, &JsValue)]) -> JsValue {
        let setup = JSON::parse(&value.to_string()).expect("JSON");
        for (half, models) in halves {
            let half = Reflect::get(&setup, &(*half).into()).expect("the half");
            Reflect::set(&half, &"models".into(), models).expect("models set");
        }
        setup
    }

    #[test]
    fn a_setup_reads_its_halves_and_the_same_models_object_is_the_same_models() {
        let (input, output) = (Object::new().into(), Object::new().into());
        let mut kept = Kept::default();
        let setup = read_setup(
            &setup_of(
                serde_json::json!({"input": {"language": "es", "end_of_turn": {"kind": "silence", "ms": 1500}}}),
                &[("input", &input)],
            ),
            &mut kept,
        )
        .expect("an input half");
        assert!(setup.output.is_none());
        let first = setup.input.expect("the input half");
        assert_eq!(
            first.config.end_of_turn,
            crate::EndOfTurn::Silence { ms: Some(1_500) }
        );
        let both = serde_json::json!({"input": {}, "output": {"voice": "ef_dora"}, "idle_unload_minutes": 0});
        let again = read_setup(
            &setup_of(both.clone(), &[("input", &input), ("output", &output)]),
            &mut kept,
        )
        .expect("both halves");
        assert!(Arc::ptr_eq(
            &first.models,
            &again.input.expect("input").models
        ));
        assert_eq!(again.config.idle_unload_minutes, 0);
        let other: JsValue = Object::new().into();
        let changed = read_setup(
            &setup_of(both, &[("input", &other), ("output", &output)]),
            &mut kept,
        )
        .expect("other models");
        assert!(!Arc::ptr_eq(
            &first.models,
            &changed.input.expect("input").models
        ));
        let unknown = serde_json::json!({"input": {"stt": {"model": "whisper"}}});
        assert!(read_setup(
            &setup_of(unknown, &[("input", &input)]),
            &mut Kept::default()
        )
        .is_err());
        let without = JSON::parse(r#"{"output": {"voice": "ef_dora"}}"#).expect("JSON");
        assert!(
            read_setup(&without, &mut Kept::default()).is_err(),
            "a half needs its models"
        );
    }

    #[test]
    async fn real_say_serialization_reaches_the_flat_web_consumer() {
        let cases = [
            (SayOutcome::Heard, "heard", None, None, None),
            (
                SayOutcome::HeardUpTo {
                    heard_chars: 2,
                    reason: StopReason::Cancelled,
                },
                "heard-up-to",
                Some(2),
                Some("cancelled"),
                None,
            ),
            (
                SayOutcome::HeardUpTo {
                    heard_chars: 2,
                    reason: StopReason::BargeIn,
                },
                "heard-up-to",
                Some(2),
                Some("barge-in"),
                None,
            ),
            (
                SayOutcome::HeardUpTo {
                    heard_chars: 2,
                    reason: StopReason::Stopped,
                },
                "heard-up-to",
                Some(2),
                Some("stopped"),
                None,
            ),
            (
                SayOutcome::NotPlayed {
                    reason: StopReason::Stopped,
                },
                "not-played",
                None,
                Some("stopped"),
                None,
            ),
            (
                SayOutcome::NotPlayed {
                    reason: StopReason::Failed {
                        code: "speech-failed".into(),
                    },
                },
                "not-played",
                None,
                Some("failed"),
                Some("speech-failed"),
            ),
        ];
        // Send real Rust outcomes through WebSaying's serializer, event listener, and
        // promise. Read only the fields in the page's flat VoiceSayOutcome contract.
        for (outcome, status, heard_chars, reason, code) in cases {
            let (messages, _message_receiver) = unbounded();
            let (send_event, receive_event) = unbounded();
            let saying = Saying::new("test".into(), messages, receive_event);
            let web = WebSaying::new(saying);
            let observed = Rc::new(RefCell::new(None));
            let received = Rc::clone(&observed);
            let listener = Closure::<dyn FnMut(JsValue)>::new(move |step| {
                *received.borrow_mut() = Some(step);
            });
            web.on_event(listener.as_ref().unchecked_ref::<Function>().clone());
            send_event
                .unbounded_send(SayEvent::Done { outcome })
                .expect("send done");
            let promised = JsFuture::from(web.outcome())
                .await
                .expect("outcome promise");
            let step = observed.borrow().clone().expect("done listener");
            let event_outcome = Reflect::get(&step, &"outcome".into()).expect("done outcome");
            for web_outcome in [event_outcome, promised] {
                let field =
                    |name: &str| Reflect::get(&web_outcome, &name.into()).expect("web field");
                assert_eq!(field("status").as_string().as_deref(), Some(status));
                assert_eq!(
                    field("heard_chars").as_f64().map(|n| n as usize),
                    heard_chars
                );
                assert_eq!(field("reason").as_string().as_deref(), reason);
                assert_eq!(field("code").as_string().as_deref(), code);
                assert!(field("reason").is_undefined() || field("reason").as_string().is_some());
            }
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

    /// Rendered speaker samples, mono at 16 kHz after gain and fading.
    pub fn rendered(&self, samples: Vec<f32>) {
        self.send(IoEvent::Rendered(samples));
    }

    /// The rendered output ended, including any fade.
    #[wasm_bindgen(js_name = playbackEnded)]
    pub fn playback_ended(&self) {
        self.send(IoEvent::PlaybackEnded);
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

    /// The microphone failed, went away, or could not open, with a stable code; the speaker goes on.
    #[wasm_bindgen(js_name = inputLost)]
    pub fn input_lost(&self, code: String) {
        self.send(IoEvent::InputLost(code));
    }

    /// The speaker failed, or the devices could not open, with a stable code; the call stops.
    pub fn failed(&self, code: String) {
        self.send(IoEvent::Failed(code));
    }
}

/// A JavaScript microphone and speaker.
struct JsIo(JsValue);

impl AudioIo for JsIo {
    fn start(&mut self, sink: IoSink, input: bool) -> Result<(), String> {
        let options = Object::new();
        let _ = Reflect::set(&options, &"input".into(), &input.into());
        self.call("start", &[sink.into(), options.into()])
    }

    fn set_input(&mut self, input: bool) {
        let _ = self.call("setInput", &[input.into()]);
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

/// A take as the page gets it, `{type: "take", data}`: its samples a `Float32Array` (a copy, the take's own dropped
/// here), the rest its JSON.
fn take_event(take: Take) -> JsValue {
    let Take {
        sample_rate,
        samples,
        started_at,
        ended_at,
        turns,
    } = take;
    let data = Object::new();
    let turns = serde_json::to_string(&turns).expect("turns serialize");
    let fields: [(&str, JsValue); 5] = [
        ("sampleRate", sample_rate.into()),
        ("samples", Float32Array::from(&samples[..]).into()),
        ("started_at", JsValue::from_f64(started_at as f64)),
        ("ended_at", JsValue::from_f64(ended_at as f64)),
        ("turns", JSON::parse(&turns).expect("JSON")),
    ];
    for (key, value) in fields {
        let _ = Reflect::set(&data, &key.into(), &value);
    }
    let event = Object::new();
    let _ = Reflect::set(&event, &"type".into(), &"take".into());
    let _ = Reflect::set(&event, &"data".into(), &data);
    event.into()
}

/// The setup in `setup`: each half's `models` taken as the page's object (the very object `kept` holds keeps the
/// models made of it), the rest read strictly as the configuration. `kept` is left with this setup's objects.
fn read_setup(setup: &JsValue, kept: &mut Kept) -> Result<VoiceSetup, JsError> {
    if !setup.is_object() {
        return Err(JsError::new("the setup is an object"));
    }
    let config: VoiceConfig = read(json_without(setup, &["input", "output"])?)?;
    let input = match half(setup, "input")? {
        None => None,
        Some((object, config)) => {
            let config: InputConfig = read(config)?;
            let models = match &kept.input {
                Some((kept, models)) if Object::is(kept, &object) => Arc::clone(models),
                _ => Arc::new(object.clone().unchecked_into::<JsInputModels>())
                    as Arc<dyn InputModels>,
            };
            Some((object, VoiceInput { models, config }))
        }
    };
    let output = match half(setup, "output")? {
        None => None,
        Some((object, config)) => {
            let config: OutputConfig = read(config)?;
            let models = match &kept.output {
                Some((kept, models)) if Object::is(kept, &object) => Arc::clone(models),
                _ => Arc::new(object.clone().unchecked_into::<JsOutputModels>())
                    as Arc<dyn OutputModels>,
            };
            Some((object, VoiceOutput { models, config }))
        }
    };
    kept.input = input
        .as_ref()
        .map(|(object, input)| (object.clone(), Arc::clone(&input.models)));
    kept.output = output
        .as_ref()
        .map(|(object, output)| (object.clone(), Arc::clone(&output.models)));
    Ok(VoiceSetup {
        input: input.map(|(_, input)| input),
        output: output.map(|(_, output)| output),
        config,
    })
}

/// The half `name` of `setup`, if it is there: its page's models object, and the rest of it as JSON.
fn half(setup: &JsValue, name: &str) -> Result<Option<(JsValue, serde_json::Value)>, JsError> {
    let value = Reflect::get(setup, &name.into()).unwrap_or(JsValue::UNDEFINED);
    if value.is_undefined() || value.is_null() {
        return Ok(None);
    }
    let models = Reflect::get(&value, &"models".into()).unwrap_or(JsValue::UNDEFINED);
    if !models.is_object() {
        return Err(JsError::new(&format!(
            "{name}.models is the page's models object"
        )));
    }
    Ok(Some((models, json_without(&value, &["models"])?)))
}

/// `object`'s own fields but `left_out`, as JSON: what is data in it, never its models.
fn json_without(object: &JsValue, left_out: &[&str]) -> Result<serde_json::Value, JsError> {
    let copy = Object::assign(&Object::new(), object.unchecked_ref());
    for key in left_out {
        let _ = Reflect::delete_property(&copy, &(*key).into());
    }
    json(&copy)
}

fn read<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Result<T, JsError> {
    serde_json::from_value(value).map_err(|error| JsError::new(&error.to_string()))
}

fn json(value: &JsValue) -> Result<serde_json::Value, JsError> {
    let text = JSON::stringify(value)
        .map_err(|_| JsError::new("not JSON"))?
        .as_string()
        .ok_or_else(|| JsError::new("not JSON"))?;
    serde_json::from_str(&text).map_err(|error| JsError::new(&error.to_string()))
}
