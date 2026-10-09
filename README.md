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
you heard. It runs models it does not know, through interfaces of its own the app fills (with
[sidevoice-engine](https://github.com/sidevoice/sidevoice-engine)'s models, say), and holds no socket: the app carries
its messages to the room and back.

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
crate at a release's git tag and compile it themselves; the web gets a WebAssembly build, published on npm as
`@sidevoice/voice`. The design is [sidevoice-core#89](https://github.com/sidevoice/sidevoice-core/issues/89).

## Using a call

The call runs models it does not know: the app supplies them through the module's own interfaces, and chooses which
model fills each slot. Natively they are Rust traits (`src/models.rs`; implement them with the re-exported
`#[async_trait]`):

| Interface | What it does |
|---|---|
| `Vad` | A voice activity detector's stream over 16 kHz mono audio: `accept(pcm)` answers one `VadFrame {end, speech, probability?}` per window; speech starts and ends where `speech` changes. `reset()` starts over. |
| `Transcriber` | `transcribe(pcm, sample_rate, language?)` → the text. |
| `Speaker` | `speak(text, voice?, language?, speed)` → mono samples and their rate. |
| `EndOfTurnModel` | Optional: `end_of_turn(pcm, sample_rate)` → the probability that the turn is over, for `smart-turn`. |
| `VoiceModels` | `load()` → `Models {vad, transcriber, speaker, end_of_turn?}`: called as the call starts; what it returned is dropped once the call has been stopped for `idle_unload_minutes`. |

A native app (the desktop app, wiring sidevoice-engine's models into these) creates a call with them, the device's
own microphone and speaker (`NativeIo`, or any other `AudioIo`) and a configuration, and runs it on its Tokio runtime:

```rust
let config: VoiceConfig = serde_json::from_value(json!({"language": "es", "voice": "ef_dora", "patience": "normal"}))?;
let (call, mut events) = VoiceCall::new(Arc::new(my_models), Box::new(NativeIo::new()), config);
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
// Other models (another transcriber, say), with the configuration they go with: call.set_models(Arc::new(other),
// config), which restarts a running call once, on both.
```

A page does the same with the WebAssembly build: `VoiceCall.create(models, io, config)`, where `models` is a
JavaScript object with `load()` answering `{vad, transcriber, speaker, endOfTurn?}`, objects with the same methods
(called through wasm-bindgen's structural imports, `src/models/web.rs`), and `io` a JavaScript microphone and speaker.
`onEvent(listener)` hears the same events as `{type, data}`, and `roomEvent(message)`, `setConfig`, `setModels`,
`setOnline`, `mute`, `cancelInput`, `start` and `stop` mirror the Rust methods (`src/web.rs`).

- **The configuration** (`VoiceConfig`, read strictly from JSON) names no model: the `language` the transcriber is
  given, the speaker's `voice` and `speed`, what ends a turn (`end_of_turn`: `silence`, or `smart-turn`, which needs
  an `EndOfTurnModel`), the `patience` (`fast`, `normal`, `calm`), the grace before a reply (`audio_grace_ms`, 1 s),
  the listening bar, and how long the models stay loaded with the call stopped (`idle_unload_minutes`, 10; 0 drops
  them as it stops). A new configuration is in effect at once.
- **`start` loads the models** if they are not loaded. They stay across stops, and are dropped once the call has been
  stopped for `idle_unload_minutes`; the next start loads them again, on the web as natively. `smart-turn` without an
  end-of-turn model refuses to start with `end-of-turn-missing`. A model that cannot load, and every other failure a
  person may be told of, is a `VoiceEvent::Error` with a stable code.
- **`AudioIo`** is the microphone and the speaker: capture arrives as 16 kHz mono samples with the echo of the call's
  own playback already cancelled, and the speaker plays a reply's chunks in order and says when each starts and ends
  (that is the clock of the heard position). It says when both are ready (`IoEvent::Ready`), and only then does the
  call listen.
- **Which models, and their tuning, are the app's.** The detector's numbers core used and this module was written
  against: a probability of 0.6, speech confirmed after 400 ms, ended after 200 ms.

## Echo cancellation

The call cancels the echo of its own playback, never relying on the operating system's (sidevoice-core#89).

- **Natively** (`NativeIo`, `src/io/native.rs`), WebRTC's AEC3 (`webrtc-audio-processing`, its C++ built with the
  crate). The microphone's samples go to mono, to 16 kHz and into 10 ms frames, and each frame passes AEC3 before
  the detector. The reference is the playback itself, taken at the moment the speaker's callback takes it (the same
  moment that moves the heard position), at the speaker's rate, to 16 kHz. Capture and reference go in step; AEC3
  estimates the delay left between them (the output buffer, the room, the input buffer). The same processing applies
  the high-pass filter, moderate noise suppression and the adaptive digital gain, as the browser does with its
  constraints. The devices' callbacks only move samples through lock-free rings; the C++ runs on the module's audio
  thread, never in a callback. A device that changes under a stream starts the canceller over; one that goes away
  stops the call with `audio-device-unavailable`.
- **In the browser**, the browser's own: the page's microphone asks `getUserMedia` for `echoCancellation`, and no
  canceller is linked into the wasm32 build, so audio is never processed twice.
- **Windows** is out of the beta: the canceller's C++ does not build there yet.

## What the call does

The call is a pure state machine (`src/call.rs`) with three regions in parallel, driven by events and a monotonic
time; a task around it (`src/voice_call.rs`) feeds it and does what it answers.

- **Listening.** Each window of the app's detector opens a turn when it is speech and loud enough: the window's level (RMS
  in dBFS, from −60 to 0, smoothed) must clear the listening bar, 0.5, raised to 0.8 while a reply plays and no turn
  is open. A turn starts with the second of audio before it, ends after the patience's silence (2, 2.5 or 3.5 s on
  top of the detector's own end), when its audio stops arriving for 5 s, or when the microphone is muted, and keeps
  at most a minute. With `smart-turn`, a pause of 0.6, 0.9 or 1.3 s (by patience) is offered to the end-of-turn model
  once; a probability of 0.5 or more ends the turn there, and a pause of 2.5, 3 or 4 s ends it anyway.
- **Recognition.** Turns are transcribed in order, one at a time, at most eight waiting. A transcript is dropped
  when it is empty, written in no Latin letter for a language that is, or too unlikely; the turn is then
  `cancelled`. Otherwise it waits the merge window (none, 0.5 or 1.5 s by patience): a turn that follows within it
  joins it, and the earlier one is reported `cancelled` with `merged`.
- **Playback.** A reply is cut into sentence chunks, each synthesized while the one before plays and never further
  ahead (two chunks at most at the speaker unplayed). A reply waits while the person's turn is open or on its way to
  the room, and for the grace after it. A turn that opens while a reply is on its way is a barge-in: the speaker
  stops with a short fade, the reply is `interrupted` (`user_interrupted`; or `unplayed`, `newer_turn`, if it had not
  sounded) and every queued reply is dropped as `unplayed` (`newer_turn`). A reply written before the person's latest
  turn (its `revision` below the turn's) is dropped as it arrives, `unplayed` (`newer_turn`), unless it is a replay
  the person asked for; one that arrives while the call is stopped is `unplayed` (`call_ended`) and never plays. A
  reply that cannot be spoken is `failed`, what of it was at the speaker is flushed, and its code is an error. A
  reply sent again under the same id is ignored, unless it is a replay.
- **The heard position** moves at chunk boundaries: a chunk counts once its last sample left the speaker, never in
  part. It is what `heard_chars` reports and what the karaoke shows.
- **Offline** is a flag, not a state: everything goes on, and a turn that starts offline is reported once, as
  `finished` with `offline`; the host's outbox keeps the messages until the room acknowledges them.

## The room's messages

The module holds no socket. It emits, each with a `client_msg_id` for the host's outbox and the room's `voice-ack`:

- `voice-user-turn {turn_id, phase: started | cancelled | finished, text?, language?, offline, started_at, ended_at?,
  merged, timings_ms?}`. The module names each turn (`turn_id`, the same in every phase; turns may overlap), and the
  room knows it by that name. `finished` is what becomes the conversation's row. A turn that started while the room
  was out of reach is reported only as `finished`, with `offline: true`.
- `voice-playback {utterance_id, status: playing | heard | interrupted | unplayed | failed, heard_chars, reason?,
  at}`, the input of the room's heard and unheard bookkeeping. `heard_chars` counts Unicode scalar values; `reason`
  is the room's word (`user_interrupted`, `newer_turn`, `call_ended`), and a failure has none.

It consumes `voice-reply {utterance_id, revision, reply_revision, thread_id, history_id, text, language, replay?}`
and the room's answer to a started turn, `voice-user-turn {phase: started, turn_id, revision}`: the revision the room
gave that turn of this call is its boundary, and a reply written below it is stale. It lets every other room message
through.

## Status

The state machine and its task, on the app's models through the module's interfaces, natively and in the browser.
The device's microphone and speaker natively, with AEC3 (macOS and Linux). Still to come: the browser's
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
  models.rs       the model interfaces the app implements (VoiceModels, Vad, Transcriber, Speaker, EndOfTurnModel);
                  models/web.rs, the page's JavaScript models through structural imports (wasm32)
  residency.rs    when idle models are dropped
  io.rs           AudioIo, the microphone and the speaker; io/native.rs, NativeIo (cpal), with io/native/output.rs
                  (what the output callback plays, and where each chunk is) and io/native/pipeline.rs (the capture
                  through the echo canceller)
  audio.rs        mono, resampling and 10 ms frames, natively
  echo.rs         echo cancellation: echo/native.rs (AEC3), echo/web.rs (the browser's)
  runtime.rs      spawning, sleeping and clocks; runtime/native.rs (Tokio), runtime/web.rs (the browser)
  web.rs          the bridge to JavaScript, only in the wasm32 build
  maybe_send.rs   Send and Sync in native builds only
tests/          fixtures/, the recorded clips the unit tests hear
build.rs        the two cfg aliases: web, native
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

## Contributing

Issues and pull requests are welcome. Read [`AGENTS.md`](AGENTS.md) first: it holds the rules for code, texts and
tests, for people and coding agents alike. Pull request titles follow
[Conventional Commits](https://www.conventionalcommits.org) (CI checks them) and become the squashed commit.

## Licence

[Apache-2.0](LICENSE). The Sidevoice name and logo are trademarks: forks are welcome under their own name — see
[`TRADEMARKS.md`](TRADEMARKS.md).
