<!-- Header: .github/assets/readme-header*.svg, from the Sidevoice brand's banner. Badges: shieldcn
     (https://shieldcn.dev), each a light/dark pair so the row follows the reader's GitHub theme. -->
<picture>
  <source media="(prefers-color-scheme: dark)" srcset=".github/assets/readme-header-on-dark.svg" />
  <img alt="Sidevoice — Give your coding agent a voice. Keep the conversation." src=".github/assets/readme-header.svg" width="750" />
</picture>

<p>
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

A page does the same with the WebAssembly build: `VoiceCall.create(engine, io, config)`, where `engine` is a
`WebEngine` of `@sidevoice/engine` and `io` a JavaScript microphone and speaker; `onEvent(listener)` hears the same
events as `{type, data}`, and `roomEvent(message)`, `setConfig`, `setOnline`, `mute`, `cancelInput`, `start` and `stop`
mirror the Rust methods (`src/web.rs`).

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
page's `WebEngine`. The engine is pinned to sidevoice-engine#71 (the `vad` capability) until a release carries it.
Still to come: the native microphone and speaker with echo cancellation (cpal and WebRTC AEC3), the browser's
(`getUserMedia` and Web Audio), and the npm package.

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

## Contributing

Issues and pull requests are welcome. Read [`AGENTS.md`](AGENTS.md) first: it holds the rules for code, texts and
tests, for people and coding agents alike. Pull request titles follow
[Conventional Commits](https://www.conventionalcommits.org) (CI checks them) and become the squashed commit.

## Licence

[Apache-2.0](LICENSE). The Sidevoice name and logo are trademarks: forks are welcome under their own name — see
[`TRADEMARKS.md`](TRADEMARKS.md).
