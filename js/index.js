// The entry point of `@sidevoice/voice`: the call's WebAssembly build as wasm-bindgen emits it (`init`, `initSync`,
// `IoSink`), the browser's microphone and speaker (`createWebAudioIo`), `VoiceCall.create(setup, options?)`, a call with
// the halves of `setup` on the page's models (voice-models.d.ts) and that microphone and speaker unless `options.io`
// brings another, and the voice seam a page drives a call through, `createVoiceHost` (voice-host.js).
import { VoiceCall as WasmVoiceCall } from "../dist/sidevoice_voice.js";
import { createWebAudioIo } from "./web-audio-io.js";
import { createSharedWebAudioIo } from "./shared-web-audio-io.js";

export { default, initSync, IoSink } from "../dist/sidevoice_voice.js";
export { createWebAudioIo, createSharedWebAudioIo };
export { createVoiceHost } from "./voice-host.js";

export const VoiceCall = {
  create(setup, options = {}) {
    const { io, ...webAudio } = options;
    return WasmVoiceCall.create(io ?? createWebAudioIo(webAudio), setup);
  },
};
