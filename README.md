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
start and stop speaking, has what you said transcribed, and reports your turn to the room as text. It takes the
agent's replies as text, has them spoken, plays them, stops when you speak over them, and reports how much of each
you heard. It runs every model through [sidevoice-engine](https://github.com/sidevoice/sidevoice-engine), local or
remote alike, and holds no socket: the app carries its messages to the room and back.

## How it fits

| Piece | Role |
|---|---|
| **sidevoice-voice** (this repository) | The call on the device: capture, echo cancellation, turns, transcription, speech, playback, barge-in, and what was heard. |
| [sidevoice-engine](https://github.com/sidevoice/sidevoice-engine) | The models: the catalogue, which build fits here, and the backends that run them (voice activity, speech to text, text to speech). |
| [sidevoice-core](https://github.com/sidevoice/sidevoice-core) | The room: the conversations, presence, routing to the agents, and the bookkeeping of what was heard. Text and events only, no audio. |
| [sidevoice-connector](https://github.com/sidevoice/sidevoice-connector) | What you install on the machine where your agents run. It gives them their voice tools and runs the core. |
| [sidevoice-desktop](https://github.com/sidevoice/sidevoice-desktop) | The app you call from: it compiles this crate in, with native capture and playback. |
| [sidevoice-web](https://github.com/sidevoice/sidevoice-web) | The call interface the app bundles; in a browser it runs this crate's WebAssembly build, `@sidevoice/voice`. |

One Rust repository, one version, shaped like sidevoice-engine. Native consumers (the desktop app) depend on the
crate at a release's git tag and compile it themselves; the web gets a WebAssembly build, published on npm as
`@sidevoice/voice`. The design is [sidevoice-core#89](https://github.com/sidevoice/sidevoice-core/issues/89).

## Using a call

A native app builds the engine (sidevoice-engine's README says how), a microphone and speaker (`AudioIo`), and a
configuration, and runs the call on its Tokio runtime:

```rust
let config: VoiceConfig = serde_json::from_value(json!({
    "vad": {"model": "silero-vad"},
    "stt": {"model": "whisper-base", "language": "es"},
    "tts": {"model": "kokoro-82m-v1.0", "voice": "ef_dora"},
    "patience": "normal",
}))?;
let (call, mut events) = VoiceCall::new(engine, io, config);
call.start(); // loads the models, opens the microphone and the speaker, listens
while let Some(event) = events.next().await {
    match event {
        VoiceEvent::RoomMessage(message) => outbox.send(message.to_json()), // to the room, kept until acknowledged
        VoiceEvent::State(state) => show(state),   // listening, recognising, playback, online
        VoiceEvent::Level(level) => meter(level),  // the microphone, from 0 to 1
        VoiceEvent::Karaoke(karaoke) => highlight(karaoke),
        VoiceEvent::Error(error) => tell(error.code),
    }
}
// What the room sends: call.room_event(RoomEvent::from_json(&message)?)
```

A page does the same with the npm package `@sidevoice/voice`, the WebAssembly build with the browser's microphone and
speaker. `engine` is a `WebEngine` of `@sidevoice/engine`, which the page brings (a peer dependency):

```js
import init, { VoiceCall } from "@sidevoice/voice";

await init();
const call = VoiceCall.create(engine, config); // the same configuration, as JSON
call.onEvent(({ type, data }) => { /* "room-message", "state", "level", "karaoke", "error" */ });
call.start(); // loads the models, asks for the microphone, listens
// What the room sends: call.roomEvent(message)
```

`onEvent(listener)` hears the same events as `{type, data}`, and `roomEvent(message)`, `setConfig`, `setOnline`,
`mute`, `cancelInput`, `start` and `stop` mirror the Rust methods (`src/web.rs`). The microphone is `getUserMedia`
with the browser's echo cancellation, noise suppression and gain control, brought to 16 kHz mono in an AudioWorklet;
the speaker is Web Audio (`js/web-audio-io.js`). `VoiceCall.create(engine, config, {outputDevice})` plays on another
output where the browser lets an `AudioContext` choose one (a non-default output may escape the browser's canceller,
an open point of sidevoice-core#89), and `{io}` brings a microphone and speaker of the page's own, with `start(sink)`,
`play(utterance, chunk, samples, sampleRate)`, `stopPlayback()` and `stop()` (`createWebAudioIo` is the default).
Their failures are errors with stable codes: `microphone-denied`, `microphone-unavailable`, `microphone-lost`,
`audio-capture-failed`, `audio-output-failed`.

- **The configuration** (`VoiceConfig`, read strictly from JSON) names the engine model of each stage (`vad`, `stt`,
  `tts`, with an optional `build`, and the language, voice and speed), what ends a turn (`end_of_turn`: `silence`, or
  `smart-turn` once the engine has it, sidevoice-engine#69), the `patience` (`fast`, `normal`, `calm`), the grace before
  a reply (`audio_grace_ms`, 1 s) and the listening bar. Local and remote models are configured alike: the engine
  has one catalogue for both, and a remote provider's key goes from the app's key store to the engine, never here.
- **`start` loads the models**, installing them if they are not; to show download progress, install them through
  the engine first. A model that cannot load, and every other failure a person may be told of, is a
  `VoiceEvent::Error` with a stable code.
- **`AudioIo`** is the microphone and the speaker: capture arrives as 16 kHz mono samples with the echo of the call's
  own playback already cancelled, and the speaker plays a reply's chunks in order and says when each starts and ends
  (that is the clock of the heard position).

## The voice seam a page drives

A page drives a call through one interface, `VoiceHost`, defined once in this package
([`js/voice-host.d.ts`](js/voice-host.d.ts)) with the payload types it carries (`VoiceSettings`, `VoiceUserTurn`,
`VoicePlayback`, `VoiceReply`, `VoiceState`, `VoiceKaraoke`, `VoiceHostError`). It has two implementations:

- **`createVoiceHost(engine, options?)`** here (`js/voice-host.js`): the call in the page, on a `WebEngine` and the
  browser's microphone and speaker (or `options.io`), with provider keys in `localStorage`
  (`localStorageProviderKeys()`, or `options.keys`; the page's engine host reads the same store for its `credential`).
- **The Sidevoice desktop app**, `window.__sidevoiceDesktop.host.voice` on macOS: the call run natively, with WebRTC
  AEC3 (sidevoice-desktop's `docs/BRIDGE.md`, "The voice call").

```js
const voice = window.__sidevoiceDesktop?.host?.voice ?? createVoiceHost(await WebEngine.create(host));
voice.onUserTurn((turn) => outbox.send({ type: "voice-user-turn", data: turn }));   // by turn.client_msg_id
voice.onPlayback((report) => outbox.send({ type: "voice-playback", data: report }));
await voice.setSettings({ stt: { model: "whisper-base", language: "es" }, tts: { model: "kokoro-82m-v1.0", voice: "ef_dora" } });
await voice.start();                                    // resolves once listening; rejects {code}
room.on("voice-reply", (data) => voice.speak(data));
```

What both promise, beyond the types:

- **`start()`** resolves at the first state whose `listening` is not `idle`, at once if the call listens already, and a
  second `start` while one is pending settles with it. It rejects with `{code}`, and with `{code: "stopped"}` when
  `stop()` comes first. `stop()` is safe at any time; `setSettings` rejects `{code}` for a model the catalogue lacks
  (`model-unknown`) or one that cannot do that stage (`model-wrong-task`).
- **Events** reach the listeners subscribed when they are emitted, in the order the call emitted them; nothing is
  buffered. For each `turn_id`, `started` comes before exactly one `finished` or `cancelled`. A turn merged into the
  next is `cancelled` with `merged`, after that next turn's `started`. For each reply, `playing` (if it sounds) comes
  before exactly one of `heard`, `interrupted`, `unplayed`, `failed`, and karaoke only follows replies given to
  `speak`.
- **Every room message carries its own `client_msg_id`**, and turns emitted while `setOnline(false)` say
  `offline: true`.
- **Settings** name a model per stage and optionally its build (one of the model's `available` builds, else
  `build-unfit`), the language, voice and speed, the patience and the end of turn (`smart-turn` only with a model of
  the `end-of-turn` capability, else `end-of-turn-unavailable`). A rejected `setSettings` changes nothing. While the
  call runs, a change to any stage restarts it (the open turn cancelled, the reply interrupted); patience and end of
  turn apply live.
- **`models()`** is `WebEngine.models()`'s list on both hosts. A remote build has `accelerator: "remote"`, and its
  `backend` is the provider id that `setProviderKey` takes (`openai`, `elevenlabs`); its `installed` says whether the
  host has that key. The engine asks the host for a key each time it needs one, so a key set takes effect at once.

## What the call does

The call is a pure state machine (`src/call.rs`) with three regions in parallel, driven by events and a monotonic
time; a task around it (`src/voice_call.rs`) feeds it and does what it answers.

- **Listening.** Each 32 ms window of the detector (the engine's `vad`, Silero: speech above a probability of 0.6,
  confirmed after 400 ms, ended after 200 ms) opens a turn when it is speech and loud enough: the window's level (RMS
  in dBFS, from −60 to 0, smoothed) must clear the listening bar, 0.5, raised to 0.8 while a reply plays and no turn
  is open. A turn starts with the second of audio before it, ends after the patience's silence (2, 2.5 or 3.5 s on
  top of the detector's own end), when its audio stops arriving for 5 s, or when the microphone is muted, and keeps
  at most a minute.
- **Recognition.** Turns are transcribed in order, one at a time, at most eight waiting. A transcript is dropped
  when it is empty, written in no Latin letter for a language that is, or too unlikely; the turn is then
  `cancelled`. Otherwise it waits the merge window (none, 0.5 or 1.5 s by patience): a turn that follows within it
  joins it, and the earlier one is reported `cancelled` with `merged`.
- **Playback.** A reply is cut into sentence chunks, each synthesized while the one before plays. A reply waits
  while the person's turn is open or on its way to the room, and for the grace after it. A turn that opens while a
  reply is on its way is a barge-in: the speaker stops with a short fade, the reply is `interrupted` (or `unplayed`
  if it had not sounded) and every queued reply is dropped as `unplayed`. A reply sent again under the same id is
  ignored, unless it is a replay.
- **The heard position** moves at chunk boundaries: a chunk counts once its last sample left the speaker, never in
  part. It is what `heard_chars` reports and what the karaoke shows.
- **Offline** is a flag, not a state: everything goes on, and turns say `offline`; the host's outbox keeps the
  messages until the room acknowledges them.

## The room's messages

The module holds no socket. It emits, each with a `client_msg_id` for the host's outbox and the room's `voice-ack`:

- `voice-user-turn {turn_id, phase: started | cancelled | finished, revision, text?, language?, offline, started_at,
  ended_at?, merged, timings?}`. `finished` is what becomes the conversation's row; `revision` is the latest room
  revision seen on a reply when the turn started.
- `voice-playback {utterance_id, status: playing | heard | interrupted | unplayed | failed, heard_chars, reason?,
  at}`, the input of the room's heard and unheard bookkeeping. `heard_chars` counts Unicode scalar values.

It consumes `voice-reply {utterance_id, revision, reply_revision, thread_id, history_id, text, language, replay?}`
and lets every other room message through.

## Status

The state machine and its task, with the engine on both platforms: natively the engine crate, in the browser the
page's `WebEngine`. The engine is pinned to sidevoice-engine#71 (`vad`, at 30fb5b0) until a release carries it.
The browser's microphone and speaker (`getUserMedia` and Web Audio) and the npm package `@sidevoice/voice`, built and
smoke-tested on every pull request and released with release-please (RELEASING.md); whether a chosen output device
stays in the browser's echo canceller is still to be checked per browser (sidevoice-core#89). Still to come: the
native microphone and speaker with echo cancellation (cpal and WebRTC AEC3).

## Layout

```
src/            the crate sidevoice-voice
  lib.rs          the front door: declares the packages, exports the public API
  call.rs         the state machine: Input and Effect, the three regions
  turns.rs        segmentation: detector windows into turns; turns/level.rs, the listening bar's level
  recognition.rs  the queue and the merge window; recognition/filter.rs, the acceptance filter
  speech.rs       a reply in sentence chunks
  playback.rs     the playback queue, barge-in and the heard position
  room.rs         the room's messages (RoomMessage, RoomEvent)
  event.rs        what a call tells its host (VoiceEvent)
  config.rs       VoiceConfig
  voice_call.rs   VoiceCall, the task around the state machine
  models.rs       the models as the task uses them; models/engine.rs (native: the engine crate),
                  models/web.rs (wasm32: the page's WebEngine)
  io.rs           AudioIo, the microphone and the speaker
  runtime.rs      spawning, sleeping and clocks; runtime/native.rs (Tokio), runtime/web.rs (the browser)
  web.rs          the bridge to JavaScript, only in the wasm32 build
  maybe_send.rs   Send and Sync in native builds only
tests/          recorded_call.rs, the whole call with real models; fixtures/, the recorded clips
build.rs        the two cfg aliases: web, native
js/             the npm package's JavaScript, shipped as it is (ES modules, no dependencies)
  index.js        the entry point: the wasm build, and VoiceCall.create(engine, config, options?) on the browser's IO
  web-audio-io.js the browser's microphone and speaker (createWebAudioIo); capture-worklet.js, its AudioWorklet
  voice-host.js   the voice seam (createVoiceHost, localStorageProviderKeys); voice-host.d.ts, VoiceHost itself
  *.d.ts          their types
npm/            the npm package's package.json (version stamped by xtask) and README
xtask/          the build tooling, `cargo xtask`: the npm package, its smoke test, the release assets, publishing
```

## Build and test

You need Rust 1.98.1 (the version `.github/actions/setup` installs). A native build compiles sidevoice-engine with
it, and so needs what the engine's does (its README, "Build and test"): CMake, a C++ compiler and libclang for
whisper.cpp, and the libstdc++ ABI line of `.cargo/config.toml` on Linux x86_64. The wasm32 build links no engine:
it reaches the page's.

The tests drive the state machine with the recorded clips of `tests/fixtures` and a detector on energy, and the task
with fake models and a fake microphone and speaker; nothing is downloaded. The whole call with real models, on the
same clips, is ignored unless asked for (about 300 MB of models the first time, kept in `$SIDEVOICE_TEST_MODELS`);
CI runs it on macOS and Linux:

```sh
cargo test --locked
cargo test --locked --test recorded_call -- --ignored --nocapture
```

The wasm32 tests run in Node and need the wasm32 target, Node.js, and the wasm-bindgen CLI at the version of
`wasm-bindgen` in `Cargo.lock` on the `PATH`:

```sh
cargo test --locked --target wasm32-unknown-unknown --lib
```

The npm package `@sidevoice/voice` is built and packed into `target/npm/` by `cargo xtask npm` (wasm-bindgen and npm
on the `PATH` too), and `cargo xtask npm-smoke` installs it as a consumer does and runs a call from it in Node, on a
fake engine and a fake microphone and speaker. The build tooling has tests of its own. How a version is released, on
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
