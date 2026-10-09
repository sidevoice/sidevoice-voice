import type { VoiceCall as WasmVoiceCall } from "../dist/sidevoice_voice.js";
import type { AudioIo, WebAudioIoOptions } from "./web-audio-io.js";

export { default, initSync, IoSink } from "../dist/sidevoice_voice.js";
export type { InitInput, InitOutput, SyncInitInput } from "../dist/sidevoice_voice.js";
export { createWebAudioIo } from "./web-audio-io.js";
export type { AudioIo, AudioIoSink, WebAudioIoOptions } from "./web-audio-io.js";

/** One voice call (`onEvent`, `start`, `stop`, `setConfig`, `roomEvent`, `setOnline`, `mute`, `cancelInput`). */
export type VoiceCall = WasmVoiceCall;

export interface VoiceCallOptions extends WebAudioIoOptions {
  /** The microphone and speaker to use instead of the browser's (`createWebAudioIo`). */
  io?: AudioIo;
}

export declare const VoiceCall: {
  /**
   * A call on `engine`'s models (a `WebEngine` of `@sidevoice/engine`) and the browser's microphone and speaker, or
   * `options.io`, set up with `config` (the configuration as JSON; a malformed one throws). It does nothing until
   * `start()`.
   */
  create(engine: unknown, config: object, options?: VoiceCallOptions): VoiceCall;
};
