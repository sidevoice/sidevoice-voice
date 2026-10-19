<!-- npm shows this file on the package page. Images by absolute URL: npm serves no file of the repository. The
     light/dark pair as in the repository's README; where the page ignores <picture>, the light one shows. -->
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/sidevoice/sidevoice-voice/main/.github/assets/readme-header-on-dark.svg" />
  <img alt="Sidevoice — Give your coding agent a voice. Keep talking." src="https://raw.githubusercontent.com/sidevoice/sidevoice-voice/main/.github/assets/readme-header.svg" width="750" />
</picture>

# @sidevoice/voice

The [Sidevoice](https://github.com/sidevoice) voice call for the web: it listens to the microphone, tells when you
start and stop speaking, has what you said transcribed, and tells the page your turns as words; it says what the
page asks it to, plays it, stops when you speak over it, and tells the page how much of it you heard. Each of the two
halves is optional: a call can only listen (a transcription try), only speak (a voice try, a call without a
microphone), or both, and it loads and opens only what its halves use. This package is its WebAssembly build
(`wasm-bindgen --target web`) with the browser's microphone and speaker. The models are the page's, supplied through
the package's own interfaces, one slot at a time (`VoiceInputModels`: a voice activity detector, a transcriber and an
optional end-of-turn classifier; `VoiceOutputModels`: a speaker; typed in `js/voice-models.d.ts`), with
[`@sidevoice/engine`](https://www.npmjs.com/package/@sidevoice/engine)'s models, say. Calls carry no application
routing identity.

Most pages use the voice seam, `createVoiceHost(source)`: `source` loads the model a slot needs (`vad()`,
`transcriber(choice)`, `speaker(choice)`, `endOfTurn(choice)`), and the host asks it only for what a call's halves
use, and only once for a choice any call holds (`js/voice-host.d.ts`):

```js
import init, { createVoiceHost } from "@sidevoice/voice";

await init();
const voice = createVoiceHost(source);
// A transcription try: the input half only, a turn ending after 1.5 s of silence. No speaker model is loaded.
const transcribing = voice.start({ input: { stt: { catalog, model, language: "es" }, endOfTurn: { kind: "silence", ms: 1500 } } });
// A voice try, or a call without a microphone: the output half only. No microphone is asked for, no detector loaded.
const listening = voice.start({ output: { tts: { catalog, model }, voice: "ef_dora", speed: 1 } });
// A normal call: both halves; `{kind: "smart", model}` loads the end-of-turn classifier, `silence` never does.
const call = voice.start({ input: { stt, endOfTurn: { kind: "smart" }, patience: "normal" }, output: { tts, voice } });
call.onTurn((turn) => { /* started, then finished (the words) or cancelled */ });
await call.ready; // the first state that is not idle; rejects { code } (a model that cannot load, the microphone, ...)
const saying = call.say("Hecho."); // a handle: saying.cancel(), saying.onEvent(step), saying.outcome
// Live, without a restart: what changes loads beside what runs and takes over at the half's safe point.
await call.update({ output: { tts, voice: "ef_other" } }); // from the next utterance; nothing playing is cut
await call.update({ input: null }); // from the next turn: the microphone closes, the call only speaks
await call.update({ input: { stt, endOfTurn: { kind: "silence" } } }); // a microphone appeared: it listens again
await call.stop(); // ends the call; its models stay loaded for `idle_unload_minutes` (10) unless another call holds them
```

`await voice.preload(settings)` loads what a call of those settings would, ahead of it (the same slots: an absent half
loads none of its own, `silence` no end-of-turn model), and opens no microphone or speaker. A call that starts with the
same choices, now or later, takes the models without loading them; they are let go `idle_unload_minutes` (10) after
the last holder, counted from the end of the preload. A page preloads the model a person has just chosen or
downloaded, so the first try or call does not wait for it; it rejects `{ code }` for a model that cannot load.

A call without an input half says `listening: "off"` in its state and has no barge-in; one without an output half
answers `say` with `not-played` (`failed`, `output-missing`). A microphone lost during a call (`onError`:
`microphone-lost`, `microphone-denied`, ...) leaves it speaking, its input half gone, instead of failing it;
`update({input})` adds the microphone back.

The lower level is one call on the page's models, `VoiceCall.create(setup, options)`:

```js
import init, { VoiceCall } from "@sidevoice/voice";

await init();
const call = VoiceCall.create({
  input: { models: { vad: () => newDetector(), transcriber: () => transcriber }, language: "es", end_of_turn: { kind: "silence" } },
  output: { models: { speaker: () => speaker }, voice: "ef_dora" },
});
call.onEvent(({ type, data }) => {
  // "turn", "state", "level", "playback-level", "error" ({ code }), "take"
});
call.start(); // loads what the halves use, asks for the microphone only with an input half, listens
const saying = call.say("Hecho.", { language: "es" });
const outcome = await saying.outcome; // heard, heard up to N characters, or not played, and why
```

The microphone is `getUserMedia` with the browser's echo cancellation, noise suppression and gain control, turned
into 16 kHz mono in an AudioWorklet; the speaker is Web Audio. `VoiceCall.create(setup, { outputDevice })` plays on
another output where the browser allows it (a non-default output may escape the browser's echo canceller), and
`{ io }` brings a microphone and speaker of the page's own (`createWebAudioIo` is the default one; it opens the
microphone only when asked, `start(sink, {input})` and `setInput(input)`). Failures are `error` events with a stable
`code` (`microphone-denied`, `microphone-unavailable`, ...), for the page to translate. `playback-level` carries one
number from 0 to 1: the smoothed RMS of the rendered speaker audio, on the same detector windows as microphone `level`
(normally 32 ms). It continues while the microphone is muted and reaches exact zero at the end of playback and on stop.
Each call of the host exposes it through `onPlaybackLevel(listener)`, which returns an unsubscribe function.

Several calls can run at once on one host: `await host.routeInput(call)` sends the next microphone frame to one call
while the others receive silence, and `host.routeInput(null)` sends silence to all. The host uses one physical
microphone and output context, the microphone open only while a call with an input half runs;
`createSharedWebAudioIo` exposes the same audio factory directly.

Source, documentation and issues: https://github.com/sidevoice/sidevoice-voice

Apache-2.0. Sidevoice is a trademark; see TRADEMARKS.md in the repository.
