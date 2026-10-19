<!-- Header: .github/assets/readme-header*.svg, from the Sidevoice brand's banner. Badges: shieldcn
     (https://shieldcn.dev), each a light/dark pair so the row follows the reader's GitHub theme. -->
<picture>
  <source media="(prefers-color-scheme: dark)" srcset=".github/assets/readme-header-on-dark.svg" />
  <img alt="Sidevoice — Give your coding agent a voice. Keep talking." src=".github/assets/readme-header.svg" width="750" />
</picture>

<p>
  <a href="https://github.com/sidevoice/sidevoice-voice/actions/workflows/release.yml"><picture><source media="(prefers-color-scheme: dark)" srcset="https://shieldcn.dev/github/ci/sidevoice/sidevoice-voice.svg?variant=secondary&size=sm&workflow=release.yml&branch=main&mode=dark" /><img alt="release status" src="https://shieldcn.dev/github/ci/sidevoice/sidevoice-voice.svg?variant=secondary&size=sm&workflow=release.yml&branch=main&mode=light" /></picture></a>
  <a href="LICENSE"><picture><source media="(prefers-color-scheme: dark)" srcset="https://shieldcn.dev/github/license/sidevoice/sidevoice-voice.svg?variant=secondary&size=sm&mode=dark" /><img alt="licence" src="https://shieldcn.dev/github/license/sidevoice/sidevoice-voice.svg?variant=secondary&size=sm&mode=light" /></picture></a>
  <picture><source media="(prefers-color-scheme: dark)" srcset="https://shieldcn.dev/badge/status-skeleton.svg?variant=secondary&size=sm&mode=dark" /><img alt="status: skeleton" src="https://shieldcn.dev/badge/status-skeleton.svg?variant=secondary&size=sm&mode=light" /></picture>
</p>

# sidevoice-voice

Reading your coding agent's plans, diffs and summaries all day is tiring. **Sidevoice** turns the exchange you
already have with your agent into a voice call. The agent keeps its context and keeps writing as usual; it also
speaks its replies, and you answer by voice and can interrupt it — from the sofa or on a walk, not only at your desk.

**sidevoice-voice** is the call itself, on the device you call from. It listens to the microphone, tells when you
start and stop speaking, has what you said transcribed, and tells the app your turns as words. It says what the app
asks it to, plays it, stops when you speak over it, and tells the app how much of it you heard. It runs models it does
not know, through interfaces of its own the app fills (with
[sidevoice-engine](https://github.com/sidevoice/sidevoice-engine)'s models, say), and carries no application routing identity. The app decides where words and playback reports go.

## How it fits

| Piece | Role |
|---|---|
| **sidevoice-voice** (this repository) | The call on the device: capture, echo cancellation, turns, transcription, speech, playback, barge-in, and what was heard. |
| [sidevoice-engine](https://github.com/sidevoice/sidevoice-engine) | The models: the catalogue, which build fits here, and the backends that run them. The apps wire its models into this module's interfaces; this module does not depend on it. |
| [sidevoice-core](https://github.com/sidevoice/sidevoice-core) | The service: sessions, presence, agent routing, and the bookkeeping of what was heard. Text and events only, no audio. |
| [sidevoice-connector](https://github.com/sidevoice/sidevoice-connector) | What you install on the machine where your agents run. It gives them their voice tools and runs the core. |
| [sidevoice-desktop](https://github.com/sidevoice/sidevoice-desktop) | The app you call from: it compiles this crate in, with native capture and playback. |
| [sidevoice-web](https://github.com/sidevoice/sidevoice-web) | The call interface the app bundles; in a browser it runs this crate's WebAssembly build, `@sidevoice/voice`. |

One Rust repository, one version, shaped like sidevoice-engine. Native consumers (the desktop app) depend on the
crate at a release's git tag and compile it themselves; the web gets a WebAssembly build on npm as `@sidevoice/voice`,
staged by each release and approved by the operator (RELEASING.md). The design is [sidevoice-core#89](https://github.com/sidevoice/sidevoice-core/issues/89).

## Using a call

A call has two halves, each optional (`VoiceSetup`): the **input** (the microphone, a voice activity detector, a
transcriber and, for `smart` end of turn, an end-of-turn classifier) and the **output** (a speaker model and the
playback). A half that is absent is neither loaded nor opened: a call that only listens (a transcription try) loads
no speaker model; one that only speaks (a voice try, a call without a microphone) asks for no microphone and loads no
detector, transcriber or end-of-turn classifier, so a blocked microphone cannot fail it. Barge-in needs both halves.

The call runs models it does not know: the app supplies them through the module's own interfaces, one slot at a time,
and chooses which model fills each slot. The call asks only for the slots its halves use, all at once as it starts:
`silence` end of turn never asks for the end-of-turn classifier. Natively they are Rust traits (`src/models.rs`;
implement them with the re-exported `#[async_trait]`):

| Interface | What it does |
|---|---|
| `Vad` | A voice activity detector's stream over 16 kHz mono audio: `accept(pcm)` answers one `VadFrame {end, speech, probability?}` per window; speech starts and ends where `speech` changes. `reset()` starts over. The call decides on each window's `probability`; a frame without one counts as its `speech`. |
| `Transcriber` | `transcribe(pcm, sample_rate, language?)` → the text. |
| `Speaker` | `speak(text, voice?, language?, speed)` → mono samples and their rate. |
| `EndOfTurnModel` | `end_of_turn(pcm, sample_rate)` → the probability that the turn is over, for `smart`. |
| `InputModels` | The input half's slots, each loaded on its own: `vad()` (a fresh stream for the call, which the call warms up on 0.1 s of silence and resets), `transcriber()`, and `end_of_turn()` (asked only for `smart`; `end-of-turn-missing` by default). |
| `OutputModels` | The output half's slot: `speaker()`. |

What the call loaded is dropped once it has been stopped for `idle_unload_minutes`.

A native app (the desktop app, wiring sidevoice-engine's models into these) creates a call with the halves it wants,
the device's own microphone and speaker (`NativeIo`, or any other `AudioIo`), and runs it on its Tokio runtime:

```rust
let setup = VoiceSetup {
    input: Some(VoiceInput {
        models: Arc::clone(&my_input_models),
        config: serde_json::from_value(json!({"language": "es", "end_of_turn": {"kind": "smart"}}))?,
    }),
    output: Some(VoiceOutput {
        models: Arc::clone(&my_output_models),
        config: serde_json::from_value(json!({"voice": "ef_dora"}))?,
    }),
    config: VoiceConfig::default(),
};
let (call, mut events) = VoiceCall::new(Box::new(NativeIo::new()), setup);
call.start(StartOptions::default()); // loads what the halves use, opens the speaker and the microphone, listens
while let Some(event) = events.next().await {
    match event {
        VoiceEvent::Turn(TurnEvent::Finished { turn_id, text, .. }) => send_words(turn_id, text),
        VoiceEvent::Turn(turn) => show_turn(turn),  // started, cancelled
        VoiceEvent::State(state) => show(state),   // listening (`off` without an input half), recognising, playback, microphone
        VoiceEvent::Level(level) => meter(level),  // the microphone, from 0 to 1
        VoiceEvent::PlaybackLevel(level) => speaker_meter(level), // rendered output, from 0 to 1
        VoiceEvent::Error(error) => tell(error.code),
        VoiceEvent::Take(take) => keep(take),      // with `StartOptions { take: true }`, as the listening ends
    }
}
// Something to say: a handle, which tells how it goes and can be cancelled.
let mut saying = call.say("Hecho, ya está en la rama.", SayOptions::default());
while let Some(step) = saying.next().await {
    match step {
        SayEvent::Playing => {}
        SayEvent::Progress { sounding, heard_chars } => highlight(sounding, heard_chars),
        SayEvent::Done { outcome } => report(outcome), // heard, heard up to N, or not played, and why
    }
}
// Another setup, live: call.update(setup). A half with the same models (the same Arc) keeps what it loaded; a running
// call loads what changes beside what it uses and swaps it at the half's safe point, with no restart (below).
```

**A running call changes without a restart** (`update`): other models, another end of turn, another voice, a half
added or removed. A half's new models load while it goes on as it was, then take over at its safe point: the input
half's at the next turn (no turn open), the output half's at the next utterance (what is being said ends in its own
voice; what is asked meanwhile waits for the new one). Nothing playing is cut. An input half added later (a microphone
appeared, the permission was granted) opens the microphone (`AudioIo::set_input`); one removed closes it. A
microphone lost under the call (`IoEvent::InputLost`: it failed or went away and no other opened, or it could not open
at all) ends the open turn with what it holds, says its code as an error, and leaves the call speaking, its input half
gone (`listening: off`), instead of failing it; without an output half there is nothing left, and the call stops.

Which microphone and speaker it uses is a `NativeDevices` handle: `NativeIo::with_devices(devices.clone())`, and the
app keeps the other clone. `devices.list()` gives the system's inputs and outputs (`{id, label}`, the id cpal's) and
the chosen ones (`None`: the system's default); `devices.choose(input, output)` changes them at any time
(`Some(None)`: back to the default; `None`: as it is). A running call switches its microphone at once and its speaker
once what it is playing has ended. A chosen device that goes away gives way to the default one, said once as `lost`
on the next list; the list is polled, cpal telling no changes.

A page does the same with the npm package `@sidevoice/voice`, the WebAssembly build with the browser's microphone and
speaker. Its models are JavaScript objects with the interfaces' methods, typed in `js/voice-models.d.ts` and called
through wasm-bindgen's structural imports (`src/models/web.rs`); the page wires its models into them (those of
`@sidevoice/engine`, say, which the page depends on itself):

```js
import init, { VoiceCall } from "@sidevoice/voice";

await init();
const call = VoiceCall.create({ // the same setup: each half's models (the page's objects) and its configuration
  input: { models: { vad: () => newDetector(), transcriber: () => transcriber /*, endOfTurn */ }, language: "es" },
  output: { models: { speaker: () => speaker }, voice: "ef_dora" },
});
call.onEvent(({ type, data }) => { /* "turn", "state", "level", "playback-level", "error", "take" */ });
call.start(); // loads what the halves use, asks for the microphone only with an input half, listens
const saying = call.say("Hecho.", { language: "es" }); // a handle: saying.cancel(), saying.onEvent(step)
const outcome = await saying.outcome; // { status: "heard" | "heard-up-to" | "not-played", ... }
```

`onEvent(listener)` hears the same events as `{type, data}`; `say(text, options)` answers a `Saying` (`id`,
`cancel()`, `onEvent(listener)` for its steps, and `outcome`, a promise); `update(setup)` (the same object as a half's
`models` is the same models), `mute`,
`cancelInput`, `endInput` (closes the open turn now with what was captured, to be told as `finished`; nothing when none
is open), `start` and `stop` mirror the Rust methods (`src/web.rs`). The microphone is `getUserMedia` with the
browser's echo cancellation, noise suppression and gain control, brought to 16 kHz mono in an AudioWorklet; the speaker
is Web Audio (`js/web-audio-io.js`). `VoiceCall.create(setup, {inputDevice, outputDevice})` captures from another
microphone, and plays on another output where
the browser lets an `AudioContext` choose one (a non-default output may escape the browser's canceller, an open point
of sidevoice-core#89), and `{io}` brings a microphone and speaker of the page's own, with `start(sink, {input})`,
`setInput(input)`, `play(utterance, chunk, samples, sampleRate)`, `stopPlayback()` and `stop()` (`createWebAudioIo`
is the default). The microphone is opened only with `input`. Their failures are errors with stable codes:
`microphone-denied`, `microphone-unavailable`, `microphone-lost`, `audio-capture-failed`, `audio-output-failed`.

- **The configuration** is data read strictly from JSON, and names no model. The input half's (`InputConfig`): the
  `language` the transcriber is given, what ends a turn (`end_of_turn`: `{"kind": "silence", "ms"?}`, a pause of `ms`
  after speech stopped, the patience's when absent; or `{"kind": "smart"}`, which needs an `EndOfTurnModel`), the
  `patience` (`fast`, `normal`, `calm`), the listening bar, and `barge_in_min_speech` (1.0 s of continuous qualified
  capture while audio sounds). The output half's (`OutputConfig`): the speaker's `voice` and `speed`, and the
  `language` of what is said when `say` gives none (the input's when absent). The call's (`VoiceConfig`): the grace
  before anything is said (`audio_grace_ms`, 1 s), and how long the models stay loaded with the call stopped
  (`idle_unload_minutes`, 10; 0 drops them as it stops).
- **`start` loads the models its halves use** if they are not loaded, every slot at once. The detector is warmed up as
  it loads, while the others still do: its first windows (a runtime's first inference is the slow one) run on 0.1 s
  of silence, then it starts over, so the first real window does not wait on it. They stay across stops, and
  are dropped once the call has been stopped for `idle_unload_minutes`; the next start loads them again, on the web as
  natively. `smart` without an end-of-turn model refuses to start with `end-of-turn-missing`. A model that cannot load,
  and every other failure a person may be told of, is a `VoiceEvent::Error` with a stable code.
- **A stop, or dropping the call, never waits on a model that does not answer**: after half a second behind one, the
  call closes the microphone and the speaker, drops the models and the tasks it started, and says it is idle; the next
  start loads the models again.
- **`AudioIo`** is the microphone and the speaker: `start(sink, input)` opens the speaker and, with `input`, the
  microphone; `set_input` opens or closes the microphone while the speaker goes on. Capture arrives as 16 kHz mono
  samples with the echo of the call's own playback already cancelled, and the speaker plays the chunks of what is
  said in order and says when each starts and ends (that is the clock of the heard position). It says when both are ready (`IoEvent::Ready`), and
  only then does the call listen.
- **Which models are the app's; when speech starts and stops is the call's.** The call times speech itself on each
  window's probability (Silero's, say), so the detector's own options only shape `speech`, which counts where a
  detector gives no probability: there, a window's `speech` should be its own decision (the shortest confirmation and
  silence a detector allows), or the detector's timing adds to the call's.

## Shared calls

A `VoiceCall` owns turn segmentation, recognition, saying handles and playback state. Several calls can run at once
without exchanging text or state. The app decides which call receives real microphone frames. A call that receives
zeros keeps its own open turn and ends it through the ordinary VAD and patience rules; routing back before that
endpoint continues the same turn. Routing never forces a transcript to finish. Continuous qualified speech still
cuts that call's sounding and queued saying handles; each handle's `done` event and flat `outcome` promise report
`heard`, `heard-up-to` or `not-played` with `reason` (`barge-in`, `cancelled`, `stopped` or `failed`) and `code` for a
failure. There is no separate cut event.

Natively, `SharedNativeAudio::new(devices)` owns one physical microphone, speaker, and AEC3 instance. `attach()`
returns `(Box<dyn AudioIo>, InputPort)` for one call. `route_input(Some(&port)).await` acknowledges a next-frame
switch, and `route_input(None).await` sends zeros to every started lane. All lanes receive the same 10 ms cadence;
only the routed lane receives real samples. Their playback chunks, progress and levels are independent. The physical
speaker receives their mixed output, which is the single AEC reference, kept 80 ms ahead of what it plays; a lane's
playback ends once all it was given has played, and stopping a lane cuts the speaker at once and mixes the other lanes'
audio again. The last active lane closes the devices.
Apps that provide their own `AudioIo` can route frames themselves.

`SharedModels::new(loader)` is a lightweight, restartable pool, slot by slot, and both halves' `InputModels` and
`OutputModels`. The loader (`SharedModelLoader`) loads one slot when no call holds it: the detector model (a
`VadFactory`), the transcriber, the speaker, the end-of-turn model. A slot one call loaded is reused by every call that
asks for it while any holds it: a transcription try's transcriber is the call's after it, and a call's speaker is not
loaded twice. Each call gets a fresh `VadFactory::new_stream()` detector. The pool retains only weak references: after
each stopped call's `idle_unload_minutes` and the final in-flight request, a slot's weights are released; the next ask
loads them again. Shared STT, TTS and end-of-turn requests are serialized so a provider with non-reentrant inference
is safe. Detector history, endpoint windows and request state remain call local.

The browser host (`js/voice-host.d.ts`) owns the shared audio and the models its calls load, slot by slot: the page's
`source` loads one slot's model for a choice (`vad()`, `transcriber(choice)`, `speaker(choice)`,
`endOfTurn(choice)`), the host asks only for what a call's halves use and only once for a choice any call holds, and
lets a model go `idle_unload_minutes` after the last call holding it stopped. The same host shape can be provided by a
native app:

```js
import init, { createVoiceHost } from "@sidevoice/voice";
await init();
const host = createVoiceHost(source);
// A transcription try: the input half only, a turn ending after 1.5 s of silence; no speaker model, no end-of-turn model.
const trying = host.start({ input: { stt: { catalog, model, language: "es" }, endOfTurn: { kind: "silence", ms: 1500 } } });
// A voice try, or a call without a microphone: the output half only; no microphone asked for, no detector.
const listening = host.start({ output: { tts: { catalog, model }, voice: "ef_dora", speed: 1 } });
// Two calls with both halves: they share every model loaded, each with its own detector stream and audio lane.
const first = host.start({ input: { stt, endOfTurn: { kind: "smart", model: turnModel } }, output: { tts, voice } });
const second = host.start({ input: { stt, endOfTurn: { kind: "smart", model: turnModel } }, output: { tts, voice } });
first.onTurn((turn) => handleFirstTurn(turn));
second.onTurn((turn) => handleSecondTurn(turn));
await Promise.all([first.ready, second.ready]); // rejects { code }: a model that cannot load, the devices refused
await host.routeInput(first); // real frames to first, zeros to second; await the worklet acknowledgement
await host.routeInput(second);
await host.routeInput(null); // zeros to both
await Promise.all([trying.stop(), listening.stop(), first.stop(), second.stop()]);
```

A call's state says `listening: "off"` without an input half; `say` without an output half is `not-played`
(`failed`, `output-missing`).

`host.preload(settings)` loads the models a call of `settings` would use, ahead of it: the same slots (an absent half loads
none of its own, `silence` no end-of-turn model), once for a choice already loaded or loading, with no microphone or
speaker opened. The pool holds them like a stopped call's: a call that starts with the same choices takes them without
loading, and they are let go `idle_unload_minutes` (10 by default, the settings' own) after the last holder, counted from
the end of the load. It resolves once every slot is loaded and rejects `{code}` for one that cannot load, keeping what
loaded.

`createSharedWebAudioIo(options)` is the browser's shared audio factory. `attach()` returns `{io, input}`;
`routeInput(input | null)` acknowledges a next-frame switch. One `getUserMedia` track and `AudioContext` serve
all lanes, the track open only while a started lane asks for the microphone; each lane has its own playback gain and telemetry processor, mixed at the context destination for the
browser's AEC. Device selection and loss reporting live on the host. The low-level
`VoiceCall.create(setup, {io?})` and `createWebAudioIo` remain for one standalone call. Browser output
selection depends on `AudioContext.setSinkId`; a non-default output can escape browser AEC on some systems.

The per-call event sequence is `started {turn_id,started_at}` and exactly one
`finished {turn_id,text,language?,started_at,ended_at,merged,timings}` or
`cancelled {turn_id,merged}`. A saying handle's `playing` and `progress` precede its one terminal `done` event.
The browser adapter flattens the serialized Rust stop reason in both `done` and `outcome`.

## Echo cancellation

The call cancels the echo of its own playback, never relying on the operating system's (sidevoice-core#89).

- **Natively** (`NativeIo`, `src/io/native.rs`), WebRTC's AEC3 (`webrtc-audio-processing`, its C++ built with the
  crate). The microphone's samples go to mono, to 16 kHz and into 10 ms frames, and each frame passes AEC3 before
  the detector. The reference is the playback itself, taken at the moment the speaker's callback takes it (the same
  moment that moves the heard position), at the speaker's rate, to 16 kHz. Capture and reference go in step; AEC3
  estimates the delay left between them (the output buffer, the room, the input buffer). The same processing applies
  the high-pass filter, moderate noise suppression and the adaptive digital gain, as the browser does with its
  constraints. The devices' callbacks only move samples through lock-free rings; the C++ runs on the module's audio
  thread, never in a callback. A device that changes under a stream starts the canceller over; a chosen one that goes
  away gives way to the default one (said once, as `lost`, with the next list); the default one going away stops the
  call with `audio-device-unavailable`.
- **In the browser**, the browser's own: the page's microphone always asks `getUserMedia` for
  `echoCancellation`; a track explicitly reporting it disabled is rejected before capture starts. The browser uses
  its rendered speaker output as the AEC reference. The playback worklet reports rendered level only; it never
  decides speech. No canceller is linked into the wasm32 build, so audio is never processed twice.
- **Windows** is out of the beta: the canceller's C++ does not build there yet.

## What the call does

The call is a pure state machine (`src/call.rs`) with three regions in parallel, driven by events and a monotonic
time; a task around it (`src/voice_call.rs`) feeds it and does what it answers.

- **Listening** is Pipecat's, as the Python core ran it (Silero, `VADParams`, smart-turn v3 and the two stop
  strategies of `pipecat-ai` 1.10.0). A window of the app's detector is speech when its probability is at least 0.6
  and the volume clears the listening bar: the loudness of the last 400 ms (ITU-R BS.1770), from −110 LUFS (0) to −10
  LUFS (1), smoothed, at least 0.5 (−60 LUFS), and 0.8 (−30 LUFS) while something plays. Speech starts after 400 ms of
  such windows and stops after a stretch without any: 0.6, 0.9 or 1.3 s by patience with `smart`, 0.2 s with
  `silence`; a breath or a click shorter than that changes nothing. A turn opens where speech starts, with the second
  of audio before it. Where speech stops a pause begins: with `smart` the end-of-turn model is asked, at every
  stop, about the turn so far (from 0.9 s before its speech started, its last 8 s); more than 0.5 ends the turn there,
  and the pause ends it anyway 2.5, 3 or 4 s after the stop. With `silence` the pause ends it its own `ms`, or 2, 2.5 or 3.5 s by
  patience, after the stop. Speech that starts again ends the pause. A turn also ends when its audio stops arriving for 5 s, and
  keeps at most a minute. Mute does not end it: a muted microphone is ordinary silence, so the turn ends by the usual
  endpoint and releasing the mute within it continues the same turn. `endInput` (`VoiceCall::end_input`) is the one way to close the
  open turn at once with what it holds; it then goes through recognition like any turn. With none open it does nothing,
  and it touches neither playback, mute nor how later turns end. Its acknowledgement means the turn is closed, not
  transcribed: the words arrive as the turn's `finished`.
- **Recognition.** Turns are transcribed in order, one at a time, at most eight waiting. A transcript is dropped
  when it is empty, or written in no Latin letter for a language that is; the turn is then
  `cancelled`. Otherwise it waits the merge window (none, 0.5 or 1.5 s by patience): a turn that follows within it
  joins it, and the earlier one is reported `cancelled` with `merged`.
- **Playback.** What the app asks to say is cut into sentence chunks, each synthesized while the one before plays and
  never further ahead (two chunks at most at the speaker unplayed). It waits while the person's turn is open or being
  transcribed, and for the grace after it. A turn that opens while something is on its way is a barge-in: the speaker
  stops with a short fade, what sounded is heard up to where it got, and everything queued is not played
  (`barge-in`). A cancel through the handle does the same to it alone (`cancelled`); a stop, to all (`stopped`), and
  what is asked while the call is stopped is not played. What cannot be spoken ends `failed` with its code, which is
  also an error, and what of it was at the speaker is flushed.
- **The heard position** moves at chunk boundaries: a chunk counts once its last sample left the speaker, never in
  part. It is what a handle's progress and outcome report as `heard_chars`.

## Turns and things said

The call knows nothing of the room; the app translates both ways.

- **Turns** (`VoiceEvent::Turn`, under the call's own `turn_id`, the same in every step): `started {turn_id,
  started_at}`, then `finished {turn_id, text, language?, started_at, ended_at, merged, timings}` (the words) or
  `cancelled {turn_id, merged}` (nothing came of it, or it joined the next turn). Turns may overlap: one can be
  transcribed while the next is spoken.
- **Things said** (`say(text, {language?})` → a `Saying` handle under the call's own id): `playing`, then
  `progress {sounding, heard_chars}` as each chunk starts and ends, then `done {outcome}`, the outcome being `heard`,
  `heard-up-to {heard_chars, reason}` or `not-played {reason}`, with `reason` one of `cancelled`, `barge-in`,
  `stopped`, `failed {code}`. `heard_chars` counts Unicode scalar values.

## Status

The state machine and its task, on the app's models through the module's interfaces, natively and in the browser.
The device's microphone and speaker natively, with AEC3 (macOS and Linux). The browser's microphone and speaker
(`getUserMedia` and Web Audio) and the npm package `@sidevoice/voice`, built and smoke-tested on every pull request
and released with release-please (RELEASING.md); whether a chosen output device stays in the browser's echo canceller
is still to be checked per browser (sidevoice-core#89).

## Layout

```
src/            the crate sidevoice-voice
  lib.rs          the front door: declares the packages, exports the public API
  call.rs         the state machine: Input and Effect, the three regions
  turns.rs        segmentation: detector windows into turns; turns/activity.rs, when speech starts and stops;
                  turns/volume.rs, the volume the listening bar is on
  level.rs        the shared microphone and playback meter
  recognition.rs  the queue and the merge window; recognition/filter.rs, the acceptance filter
  speech.rs       a text in sentence chunks
  playback.rs     the playback queue, barge-in and the heard position
  event.rs        what a call tells its host (VoiceEvent, TurnEvent)
  say.rs          something said: SayOptions, the Saying handle, SayEvent and SayOutcome
  setup.rs        VoiceSetup: the call's halves, VoiceInput and VoiceOutput, each optional
  config.rs       VoiceConfig, InputConfig, OutputConfig
  voice_call.rs   VoiceCall, the task around the state machine
  models.rs       the model interfaces the app implements (InputModels, OutputModels, Vad, Transcriber, Speaker,
                  EndOfTurnModel) and SharedModels, the pool slot by slot; models/web.rs, the page's JavaScript models
                  through structural imports (wasm32)
  residency.rs    when idle models are dropped
  io.rs           AudioIo, the microphone and the speaker; io/native.rs, NativeIo (cpal), with io/native/output.rs
                  (what the output callback plays, and where each chunk is), io/native/playout.rs (the audio thread's
                  speaker side: one resampler per stream, the playback ring, the meter and chunk reports),
                  io/native/pipeline.rs (the capture through the echo canceller) and io/native/devices.rs
                  (NativeDevices: which devices, listed and chosen)
  audio.rs        mono, resampling and 10 ms frames, natively
  echo.rs         echo cancellation: echo/native.rs (AEC3), echo/web.rs (the browser's)
  runtime.rs      spawning, sleeping and clocks; runtime/native.rs (Tokio), runtime/web.rs (the browser)
  web.rs          the bridge to JavaScript, only in the wasm32 build
  maybe_send.rs   Send and Sync in native builds only
tests/          fixtures/, the recorded clips the unit tests hear
build.rs        the two cfg aliases: web, native
js/             the npm package's JavaScript, shipped as it is (ES modules, no dependencies)
  index.js        the entry point: the wasm build, and VoiceCall.create(setup, options?) on the browser's IO
  voice-models.d.ts the model interfaces the page implements (VoiceInputModels, VoiceOutputModels, VoiceVad,
                  VoiceTranscriber, VoiceSpeaker, VoiceEndOfTurn)
  web-audio-io.js the browser's microphone and speaker (createWebAudioIo); capture-worklet.js and
                  playback-worklet.js, their AudioWorklets
  voice-host.js   the voice seam (createVoiceHost); voice-host.d.ts, VoiceHost itself
  *.d.ts          their types
npm/            the npm package's package.json (version stamped by xtask) and README
xtask/          the build tooling, `cargo xtask`: the npm package, its smoke test, the release assets, publishing
```

## Build and test

You need Rust 1.98.1 (the version `.github/actions/setup` installs). The module links no model runtime. Natively,
WebRTC's audio processing needs meson, ninja and pkg-config, and cpal needs ALSA's development files on Linux
(`libasound2-dev`); the wasm32 build links no canceller.

The tests drive the state machine with the recorded clips of `tests/fixtures` and a detector on energy, and the task
with fake models (a fake end-of-turn classifier among them) and a fake microphone and speaker; nothing is downloaded.
The echo canceller runs on the same clips: one plays as the reply and comes back through a room with reflections, the
other speaks over it. The native microphone and speaker are tested without devices.
The call with real models is the apps' to run: they wire the models in, and their CI fails when a model and this
module do not fit.

```sh
cargo test --locked
```

The wasm32 tests run in Node and need the wasm32 target, Node.js, and the wasm-bindgen CLI at the version of
`wasm-bindgen` in `Cargo.lock` on the `PATH`:

```sh
cargo test --locked --target wasm32-unknown-unknown --lib
```

The npm package `@sidevoice/voice` is built and packed into `target/npm/` by `cargo xtask npm` (wasm-bindgen and npm
on the `PATH` too), and `cargo xtask npm-smoke` installs it as a consumer does and runs a call from it in Node, on a
fake models and a fake microphone and speaker. The build tooling has tests of its own. How a version is released, on
GitHub and npm, is in [`RELEASING.md`](RELEASING.md).

```sh
cargo test --locked --manifest-path xtask/Cargo.toml
cargo xtask npm
cargo xtask npm-smoke
```

## Contributing

Issues and pull requests are welcome. Read [`AGENTS.md`](AGENTS.md) first: it holds the rules for code, texts and
tests, for people and coding agents alike. Pull request titles follow
[Conventional Commits](https://www.conventionalcommits.org) (CI checks them) and become the squashed commit.

## Licence

[Apache-2.0](LICENSE). The Sidevoice name and logo are trademarks: forks are welcome under their own name — see
[`TRADEMARKS.md`](TRADEMARKS.md).
