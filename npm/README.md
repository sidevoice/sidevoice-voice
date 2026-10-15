<!-- npm shows this file on the package page. Images by absolute URL: npm serves no file of the repository. The
     light/dark pair as in the repository's README; where the page ignores <picture>, the light one shows. -->
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/sidevoice/sidevoice-voice/main/.github/assets/readme-header-on-dark.svg" />
  <img alt="Sidevoice — Give your coding agent a voice. Keep talking." src="https://raw.githubusercontent.com/sidevoice/sidevoice-voice/main/.github/assets/readme-header.svg" width="750" />
</picture>

# @sidevoice/voice

The [Sidevoice](https://github.com/sidevoice) voice call for the web: it listens to the microphone, tells when you
start and stop speaking, has what you said transcribed, and tells the page your turns as words; it says what the
page asks it to, plays it, stops when you speak over it, and tells the page how much of it you heard. This package
is its WebAssembly build (`wasm-bindgen --target web`) with the browser's microphone and speaker. The models are the
page's, supplied through the package's own interfaces (`VoiceModels`: a voice activity detector, a transcriber, a
speaker and an optional end-of-turn classifier, typed in `js/voice-models.d.ts`), with
[`@sidevoice/engine`](https://www.npmjs.com/package/@sidevoice/engine)'s models, say. Calls carry no application routing identity.

```js
import init, { VoiceCall } from "@sidevoice/voice";

await init();
// The page's models: `load()` answers objects with the methods js/voice-models.d.ts types.
const models = { load: async () => ({ vad, transcriber, speaker }) };
const call = VoiceCall.create(models, { language: "es", voice: "ef_dora", patience: "normal" });
call.onEvent(({ type, data }) => {
  // "turn", "state", "level", "playback-level", "error" ({ code })
});
call.start(); // loads the models, asks for the microphone, listens
const saying = call.say("Hecho.", { language: "es" }); // a handle: saying.cancel(), saying.onEvent(step)
const outcome = await saying.outcome; // heard, heard up to N characters, or not played, and why
```

The microphone is `getUserMedia` with the browser's echo cancellation, noise suppression and gain control, turned
into 16 kHz mono in an AudioWorklet; the speaker is Web Audio. `VoiceCall.create(models, config, { outputDevice })`
plays on another output where the browser allows it (a non-default output may escape the browser's echo canceller),
and `{ io }` brings a microphone and speaker of the page's own (`createWebAudioIo` is the default one). Failures are
`error` events with a stable `code` (`microphone-denied`, `microphone-unavailable`, ...), for the page to translate.
`playback-level` carries one number from 0 to 1: the smoothed RMS of the rendered speaker audio, on the same
detector windows as microphone `level` (normally 32 ms). It continues while the microphone is muted and reaches
exact zero at the end of playback and on stop. Each `createVoiceHost().createCall()` exposes it through
`onPlaybackLevel(listener)`, which returns an unsubscribe function.

Source, documentation and issues: https://github.com/sidevoice/sidevoice-voice

Apache-2.0. Sidevoice is a trademark; see TRADEMARKS.md in the repository.

For several concurrent calls, use `createVoiceHost(source)`, then `host.createCall()` for each call.
Start them independently and `await host.routeInput(call)` to send the next microphone frame to one call while
the others receive silence. `host.routeInput(null)` sends silence to all. The host uses one physical microphone and
output context; `createSharedWebAudioIo` exposes the same audio factory directly. `source.models(settings).load()`
provides shared `transcriber`/`speaker` and `createVad()` for a fresh detector stream per call.
