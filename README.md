<!-- Header: .github/assets/readme-header*.svg, from the Sidevoice brand's banner. Badges: shieldcn
     (https://shieldcn.dev), each a light/dark pair so the row follows the reader's GitHub theme. -->
<picture>
  <source media="(prefers-color-scheme: dark)" srcset=".github/assets/readme-header-on-dark.svg" />
  <img alt="Sidevoice — Give your coding agent a voice. Keep the conversation." src=".github/assets/readme-header.svg" width="750" />
</picture>

<p>
  <a href="https://github.com/sidevoice/sidevoice-voice/actions/workflows/release.yml"><picture><source media="(prefers-color-scheme: dark)" srcset="https://shieldcn.dev/github/ci/sidevoice/sidevoice-voice.svg?variant=secondary&size=sm&workflow=release.yml&branch=main&mode=dark" /><img alt="release status" src="https://shieldcn.dev/github/ci/sidevoice/sidevoice-voice.svg?variant=secondary&size=sm&workflow=release.yml&branch=main&mode=light" /></picture></a>
  <a href="LICENSE"><picture><source media="(prefers-color-scheme: dark)" srcset="https://shieldcn.dev/github/license/sidevoice/sidevoice-voice.svg?variant=secondary&size=sm&mode=dark" /><img alt="licence" src="https://shieldcn.dev/github/license/sidevoice/sidevoice-voice.svg?variant=secondary&size=sm&mode=light" /></picture></a>
  <picture><source media="(prefers-color-scheme: dark)" srcset="https://shieldcn.dev/badge/status-skeleton.svg?variant=secondary&size=sm&mode=dark" /><img alt="status: skeleton" src="https://shieldcn.dev/badge/status-skeleton.svg?variant=secondary&size=sm&mode=light" /></picture>
</p>

# sidevoice-voice

Reading your coding agent's plans, diffs and summaries all day is tiring. **Sidevoice** turns the conversation you
already have with your agent into a voice call. The agent keeps its context and keeps writing as usual; it also
speaks its replies, and you answer by voice and can interrupt it — from the sofa or on a walk, not only at your desk.

**sidevoice-voice** is the call itself, on the device you call from. It listens to the microphone, tells when you
start and stop speaking, has what you said transcribed, and tells the app your turns as words. It says what the app
asks it to, plays it, stops when you speak over it, and tells the app how much of it you heard. It runs models it does
not know, through interfaces of its own the app fills (with
[sidevoice-engine](https://github.com/sidevoice/sidevoice-engine)'s models, say), and knows nothing of the room:
only the app talks to it, and the app carries what it hears to the room and what the room sends to it.

## How it fits

| Piece | Role |
|---|---|
| **sidevoice-voice** (this repository) | The call on the device: capture, echo cancellation, turns, transcription, speech, playback, barge-in, and what was heard. |
| [sidevoice-engine](https://github.com/sidevoice/sidevoice-engine) | The models: the catalogue, which build fits here, and the backends that run them. The apps wire its models into this module's interfaces; this module does not depend on it. |
| [sidevoice-core](https://github.com/sidevoice/sidevoice-core) | The room: the conversations, presence, routing to the agents, and the bookkeeping of what was heard. Text and events only, no audio. |
| [sidevoice-connector](https://github.com/sidevoice/sidevoice-connector) | What you install on the machine where your agents run. It gives them their voice tools and runs the core. |
| [sidevoice-desktop](https://github.com/sidevoice/sidevoice-desktop) | The app you call from: it compiles this crate in, with native capture and playback. |
| [sidevoice-web](https://github.com/sidevoice/sidevoice-web) | The call interface the app bundles; in a browser it runs this crate's WebAssembly build, `@sidevoice/voice`. |

One Rust repository, one version, shaped like sidevoice-engine. Native consumers (the desktop app) depend on the
crate at a release's git tag and compile it themselves; the web gets a WebAssembly build on npm as `@sidevoice/voice`,
staged by each release and approved by the operator (RELEASING.md). The design is [sidevoice-core#89](https://github.com/sidevoice/sidevoice-core/issues/89).

## Using a call

The call runs models it does not know: the app supplies them through the module's own interfaces, and chooses which
model fills each slot. Natively they are Rust traits (`src/models.rs`; implement them with the re-exported
`#[async_trait]`):

| Interface | What it does |
|---|---|
| `Vad` | A voice activity detector's stream over 16 kHz mono audio: `accept(pcm)` answers one `VadFrame {end, speech, probability?}` per window; speech starts and ends where `speech` changes. `reset()` starts over. The call decides on each window's `probability`; a frame without one counts as its `speech`. |
| `Transcriber` | `transcribe(pcm, sample_rate, language?)` → the text. |
| `Speaker` | `speak(text, voice?, language?, speed)` → mono samples and their rate. |
| `EndOfTurnModel` | Optional: `end_of_turn(pcm, sample_rate)` → the probability that the turn is over, for `smart-turn`. |
| `VoiceModels` | `load()` → `Models {vad, transcriber, speaker, end_of_turn?}`: called as the call starts; what it returned is dropped once the call has been stopped for `idle_unload_minutes`. |

A native app (the desktop app, wiring sidevoice-engine's models into these) creates a call with them, the device's
own microphone and speaker (`NativeIo`, or any other `AudioIo`) and a configuration, and runs it on its Tokio runtime:

```rust
let config: VoiceConfig = serde_json::from_value(json!({"language": "es", "voice": "ef_dora", "patience": "normal"}))?;
let (call, mut events) = VoiceCall::new(Arc::new(my_models), Box::new(NativeIo::new()), config);
call.start(StartOptions::default()); // loads the models, opens the microphone and the speaker, listens
while let Some(event) = events.next().await {
    match event {
        VoiceEvent::Turn(TurnEvent::Finished { turn_id, text, .. }) => send_words(turn_id, text),
        VoiceEvent::Turn(turn) => show_turn(turn),  // started, cancelled
        VoiceEvent::State(state) => show(state),   // listening, recognising, playback, microphone
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
// Other models (another transcriber, say), with the configuration they go with: call.set_models(Arc::new(other),
// config), which restarts a running call once, on both.
```

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
const models = { load: async () => ({ vad, transcriber, speaker /*, endOfTurn */ }) }; // the page's VoiceModels
const call = VoiceCall.create(models, config); // the same configuration, as JSON
call.onEvent(({ type, data }) => { /* "turn", "state", "level", "playback-level", "error", "take" */ });
call.start(); // loads the models, asks for the microphone, listens
const saying = call.say("Hecho.", { language: "es" }); // a handle: saying.cancel(), saying.onEvent(step)
const outcome = await saying.outcome; // { status: "heard" | "heard-up-to" | "not-played", ... }
```

`onEvent(listener)` hears the same events as `{type, data}`; `say(text, options)` answers a `Saying` (`id`,
`cancel()`, `onEvent(listener)` for its steps, and `outcome`, a promise); `setConfig`, `setModels`, `mute`,
`cancelInput`, `start` and `stop` mirror the Rust methods (`src/web.rs`). The microphone is `getUserMedia` with the
browser's echo cancellation, noise suppression and gain control, brought to 16 kHz mono in an AudioWorklet; the speaker
is Web Audio (`js/web-audio-io.js`). `VoiceCall.create(models, config, {inputDevice, outputDevice})` captures from another
microphone, and plays on another output where
the browser lets an `AudioContext` choose one (a non-default output may escape the browser's canceller, an open point
of sidevoice-core#89), and `{io}` brings a microphone and speaker of the page's own, with `start(sink)`,
`play(utterance, chunk, samples, sampleRate)`, `stopPlayback()` and `stop()` (`createWebAudioIo` is the default).
Their failures are errors with stable codes: `microphone-denied`, `microphone-unavailable`, `microphone-lost`,
`audio-capture-failed`, `audio-output-failed`.

- **The configuration** (`VoiceConfig`, read strictly from JSON) names no model: the `language` the transcriber is
  given, the speaker's `voice` and `speed`, what ends a turn (`end_of_turn`: `silence`; `smart-turn`, which needs
  an `EndOfTurnModel`; or `manual`, where no pause does), the `patience` (`fast`, `normal`, `calm`), the grace before
  anything is said (`audio_grace_ms`, 1 s), the listening bar, and how long the models stay loaded with the call
  stopped (`idle_unload_minutes`, 10; 0 drops them as it stops). A new configuration is in effect at once.
- **`start` loads the models** if they are not loaded. They stay across stops, and are dropped once the call has been
  stopped for `idle_unload_minutes`; the next start loads them again, on the web as natively. `smart-turn` without an
  end-of-turn model refuses to start with `end-of-turn-missing`. A model that cannot load, and every other failure a
  person may be told of, is a `VoiceEvent::Error` with a stable code.
- **A stop, or dropping the call, never waits on a model that does not answer**: after half a second behind one, the
  call closes the microphone and the speaker, drops the models and the tasks it started, and says it is idle; the next
  start loads the models again.
- **`AudioIo`** is the microphone and the speaker: capture arrives as 16 kHz mono samples with the echo of the call's
  own playback already cancelled, and the speaker plays the chunks of what is said in order and says when each
  starts and ends (that is the clock of the heard position). It says when both are ready (`IoEvent::Ready`), and
  only then does the call listen.
- **Which models are the app's; when speech starts and stops is the call's.** The call times speech itself on each
  window's probability (Silero's, say), so the detector's own options only shape `speech`, which counts where a
  detector gives no probability: there, a window's `speech` should be its own decision (the shortest confirmation and
  silence a detector allows), or the detector's timing adds to the call's.

## The voice seam a page drives

A page drives a call through one interface, `VoiceHost`, defined once in this package
([`js/voice-host.d.ts`](js/voice-host.d.ts)) with the types it carries (`VoiceSettings`, `VoiceHostError`, and from
[`js/voice-events.d.ts`](js/voice-events.d.ts) `VoiceTurnEvent`, `VoiceCallState`, `VoiceSaying`, `VoiceSayEvent`,
`VoiceSayOutcome`, `VoiceTake`). The call behind it knows nothing of the room: the page translates both ways. It has two
implementations:

- **`createVoiceHost(source, options?)`** here (`js/voice-host.js`): the call in the page, on the page's models and the
  browser's microphone and speaker (or `options.io`). `source` is the page's `VoiceModelSource`: `models(settings)`,
  the `VoiceModels` (`js/voice-models.d.ts`) that fill the call's slots for the settings, or a refusal by `{code}`.
  Which models there are, which fills each slot, how a setting names it and the keys any of them needs are the page's;
  this package lists none and names none.
- **The Sidevoice desktop app**, `window.__sidevoiceDesktop.host.voice` on macOS: the call run natively, with WebRTC
  AEC3 (sidevoice-desktop's `docs/BRIDGE.md`, "The voice call").

```js
const voice = window.__sidevoiceDesktop?.host?.voice ?? createVoiceHost(source); // source: the page's models
voice.onTurn((turn) => turn.phase === "finished" && sendWords(turn.turn_id, turn.text)); // the page's to send
await voice.setSettings({ stt: { ...sttChoice, language: "es" }, tts: { ...ttsChoice, voice: "ef_dora" } }); // choices: the page's
await voice.start();                                    // resolves once listening; rejects {code}
const saying = voice.say("Hecho, ya está en la rama.", { language: "es" });
saying.onEvent((step) => step.type === "progress" && highlight(step.sounding, step.heard_chars));
const outcome = await saying.outcome; // heard, heard up to N characters, or not played; the page reports it
// saying.cancel() stops it, when the page learns it is no longer to be said.
```

What both promise, beyond the types:

- **`start()`** resolves once the microphone and the speaker work: at the first state whose `listening` is not `idle`,
  at once if the call listens already, and a second `start` while one is pending settles with it. It rejects with
  `{code}`, and with `{code: "stopped"}` when `stop()` comes first; `smart-turn` without an end-of-turn model rejects
  `end-of-turn-missing`. `stop()` is safe at any time, and a `start` right after it, awaited or not, starts the call
  again. `mute` holds from the first call, even before `setSettings`. Settings that change the models give the call
  the new models and configuration together (a live call restarts once); `setSettings` rejects with the model
  source's `{code}` for settings it cannot fill.
- **Turns** reach the listeners subscribed when they are emitted, in the order the call emitted them, with the state,
  microphone and playback levels and errors; nothing is buffered. `playback-level` is a bare number from 0 to 1,
  computed from audio rendered by the speaker after gain and fading, on the same detector windows as `level`
  (normally 512 samples at 16 kHz, or 32 ms). It continues while the microphone is muted, is zero in silence,
  and emits exact zero when playback ends or the call stops. `VoiceHost.onPlaybackLevel(listener)` subscribes to this
  number; its unsubscribe removes only that listener, and subscriptions survive stop/start. Each turn has the call's
  own `turn_id`: `started` comes before exactly one
  `finished` (the words) or `cancelled`. A turn merged into the next is `cancelled` with `merged`, after that next
  turn's `started`.
  The state says `speaking` while the person speaks in an open turn, and `listening` again when they pause in it (their
  speech stopped, as the detector decides it) or it ends; its end shows as `recognising` rises.
- **A take** (`VoiceTake`) is what the microphone gave from the moment the call listened until that listening ended,
  as the call heard it (16 kHz mono, after echo cancellation; silence while muted), with where each turn's speech
  started, paused and was ended, in milliseconds from its first sample at the call's own positions in the audio. It is
  kept only for a `start({take: true})`, at most two minutes of it, in memory, and handed to `onTake` once as that
  listening ends: before `stop()` resolves, before the error that ended it, before a restart on other models listens
  again. The call keeps no copy, and never records otherwise.
- **`say(text, options?)`** answers a handle at once: its `id`, `cancel()`, `onEvent(listener)` for its steps in order
  (`playing`, `progress` as each chunk starts and ends, then `done`), and `outcome`, a promise of how it ended: `heard`,
  `heard-up-to` with `heard_chars`, or `not-played`, the reason being `cancelled`, `barge-in`, `stopped` or `failed`
  with a code. What is said before `setSettings`, or with the call stopped, is `not-played` (`stopped`).
- **Settings** are a choice per slot, `stt` and `tts`, with the patience, the end of turn and `idle_unload_minutes`
  (how long the models stay loaded with the call stopped; 10 by default). Of a slot the call reads only the language
  (`stt`) and the voice and speed (`tts`); everything else in it names the model as the page's source does, and goes to
  the source as the page wrote it. A rejected `setSettings` changes nothing. A change to what chooses models (any other
  field of a slot, or the end of turn) gives the call other models, which restarts it while it runs (the open turn
  cancelled, what is being said stopped); the language, voice, speed, patience and idle minutes apply live.
- **Audio devices.** `audioDevices()` lists the microphones and outputs (`{id, label}`; the system's default is the
  choice `null`, not an entry) and which are chosen, and whether the output can be chosen here (`outputSelectable`:
  in a browser, `AudioContext.setSinkId`). `setAudioDevices({input?, output?})` chooses either at any time: a call that
  runs switches without ending (the new microphone open before the old one is let go), and the choice holds for every
  call after. An output that cannot be chosen is refused (`audio-output-unselectable`), never pretended.
  `onAudioDevices(listener)` hears the list each time it changes, and when a chosen device goes away: the default one
  is in use from then, and that list says which was lost (`lost`). Only the default microphone going away ends a call
  (`microphone-lost`).

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
- **In the browser**, the browser's own: the page's microphone asks `getUserMedia` for `echoCancellation`, and no
  canceller is linked into the wasm32 build, so audio is never processed twice.
- **Windows** is out of the beta: the canceller's C++ does not build there yet.

## What the call does

The call is a pure state machine (`src/call.rs`) with three regions in parallel, driven by events and a monotonic
time; a task around it (`src/voice_call.rs`) feeds it and does what it answers.

- **Listening** is Pipecat's, as the Python core ran it (Silero, `VADParams`, smart-turn v3 and the two stop
  strategies of `pipecat-ai` 1.10.0). A window of the app's detector is speech when its probability is at least 0.6
  and the volume clears the listening bar: the loudness of the last 400 ms (ITU-R BS.1770), from −110 LUFS (0) to −10
  LUFS (1), smoothed, at least 0.5 (−60 LUFS), and 0.8 (−30 LUFS) while something plays. Speech starts after 400 ms of
  such windows and stops after a stretch without any: 0.6, 0.9 or 1.3 s by patience with `smart-turn`, 0.2 s with
  `silence` and `manual`; a breath or a click shorter than that changes nothing. A turn opens where speech starts,
  with the second of audio before it. Where speech stops a pause begins: with `smart-turn` the end-of-turn model is
  asked, at every stop, about the turn so far (from 0.9 s before its speech started, its last 8 s); more than 0.5 ends
  the turn there, and the pause ends it anyway 2.5, 3 or 4 s after the stop. With `silence` the pause ends it 2, 2.5
  or 3.5 s after the stop; with `manual` no pause ends it. Speech that starts again ends the pause. A turn also ends
  when its audio stops arriving for 5 s or the microphone is muted, and keeps at most a minute.
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
  config.rs       VoiceConfig
  voice_call.rs   VoiceCall, the task around the state machine
  models.rs       the model interfaces the app implements (VoiceModels, Vad, Transcriber, Speaker, EndOfTurnModel);
                  models/web.rs, the page's JavaScript models through structural imports (wasm32)
  residency.rs    when idle models are dropped
  io.rs           AudioIo, the microphone and the speaker; io/native.rs, NativeIo (cpal), with io/native/output.rs
                  (what the output callback plays, and where each chunk is), io/native/pipeline.rs (the capture
                  through the echo canceller) and io/native/devices.rs (NativeDevices: which devices, listed and
                  chosen)
  audio.rs        mono, resampling and 10 ms frames, natively
  echo.rs         echo cancellation: echo/native.rs (AEC3), echo/web.rs (the browser's)
  runtime.rs      spawning, sleeping and clocks; runtime/native.rs (Tokio), runtime/web.rs (the browser)
  web.rs          the bridge to JavaScript, only in the wasm32 build
  maybe_send.rs   Send and Sync in native builds only
tests/          fixtures/, the recorded clips the unit tests hear
build.rs        the two cfg aliases: web, native
js/             the npm package's JavaScript, shipped as it is (ES modules, no dependencies)
  index.js        the entry point: the wasm build, and VoiceCall.create(models, config, options?) on the browser's IO
  voice-models.d.ts the model interfaces the page implements (VoiceModels, VoiceVad, VoiceTranscriber, VoiceSpeaker,
                  VoiceEndOfTurn)
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
