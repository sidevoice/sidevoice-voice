<!-- npm shows this file on the package page. Images by absolute URL: npm serves no file of the repository. The
     light/dark pair as in the repository's README; where the page ignores <picture>, the light one shows. -->
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/sidevoice/sidevoice-voice/main/.github/assets/readme-header-on-dark.svg" />
  <img alt="Sidevoice — Give your coding agent a voice. Keep the conversation." src="https://raw.githubusercontent.com/sidevoice/sidevoice-voice/main/.github/assets/readme-header.svg" width="750" />
</picture>

# @sidevoice/voice

The [Sidevoice](https://github.com/sidevoice) voice call for the web: it listens to the microphone, tells when you
start and stop speaking, has what you said transcribed, and hands your turn to the page as a message for the room;
it takes the room's replies, has them spoken, plays them, stops when you speak over them, and reports how much of
each you heard. This package is its WebAssembly build (`wasm-bindgen --target web`) with the browser's microphone and
speaker. The models are the page's, supplied through the package's own interfaces (`VoiceModels`: a voice activity
detector, a transcriber, a speaker and an optional end-of-turn classifier, typed in `js/voice-models.d.ts`), with
[`@sidevoice/engine`](https://www.npmjs.com/package/@sidevoice/engine)'s models, say; and the call holds no socket:
the page carries its messages to the room and back.

```js
import init, { VoiceCall } from "@sidevoice/voice";

await init();
// The page's models: `load()` answers objects with the methods js/voice-models.d.ts types.
const models = { load: async () => ({ vad, transcriber, speaker }) };
const call = VoiceCall.create(models, { language: "es", voice: "ef_dora", patience: "normal" });
call.onEvent(({ type, data }) => {
  // "room-message" (to the room, kept until acknowledged), "state", "level", "karaoke", "error" ({ code })
});
call.start(); // loads the models, asks for the microphone, listens
socket.onmessage = ({ data }) => call.roomEvent(JSON.parse(data));
```

The microphone is `getUserMedia` with the browser's echo cancellation, noise suppression and gain control, turned
into 16 kHz mono in an AudioWorklet; the speaker is Web Audio. `VoiceCall.create(models, config, { outputDevice })`
plays on another output where the browser allows it (a non-default output may escape the browser's echo canceller),
and `{ io }` brings a microphone and speaker of the page's own (`createWebAudioIo` is the default one). Failures are
`error` events with a stable `code` (`microphone-denied`, `microphone-unavailable`, ...), for the page to translate.

Source, documentation and issues: https://github.com/sidevoice/sidevoice-voice

Apache-2.0. Sidevoice is a trademark; see TRADEMARKS.md in the repository.
