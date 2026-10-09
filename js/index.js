// The entry point of `@sidevoice/voice`: the call's WebAssembly build as wasm-bindgen emits it (`init`, `initSync`,
// `IoSink`), the browser's microphone and speaker (`createWebAudioIo`), and `VoiceCall.create(models, config,
// options?)`, a call on the page's models (voice-models.d.ts) and that microphone and speaker unless `options.io`
// brings another.
import { VoiceCall as WasmVoiceCall } from "../dist/sidevoice_voice.js";
import { createWebAudioIo } from "./web-audio-io.js";

export { default, initSync, IoSink } from "../dist/sidevoice_voice.js";
export { createWebAudioIo };

export const VoiceCall = {
  create(models, config, options = {}) {
    const { io, ...webAudio } = options;
    return WasmVoiceCall.create(models, io ?? createWebAudioIo(webAudio), config);
  },
};
