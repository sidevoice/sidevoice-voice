// The entry point of `@sidevoice/voice`: the call's WebAssembly build as wasm-bindgen emits it (`init`, `initSync`,
// `IoSink`), the browser's microphone and speaker (`createWebAudioIo`), and `VoiceCall.create(engine, config,
// options?)`, a call on that microphone and speaker unless `options.io` brings another.
import { VoiceCall as WasmVoiceCall } from "../dist/sidevoice_voice.js";
import { createWebAudioIo } from "./web-audio-io.js";

export { default, initSync, IoSink } from "../dist/sidevoice_voice.js";
export { createWebAudioIo };

export const VoiceCall = {
  create(engine, config, options = {}) {
    const { io, ...webAudio } = options;
    return WasmVoiceCall.create(engine, io ?? createWebAudioIo(webAudio), config);
  },
};
