import type { VoiceCall as WasmVoiceCall } from "../dist/sidevoice_voice.js";
import type { AudioIo, WebAudioIoOptions } from "./web-audio-io.js";
import type { VoiceModels } from "./voice-models.js";

export { default, initSync, IoSink } from "../dist/sidevoice_voice.js";
export type { InitInput, InitOutput, SyncInitInput } from "../dist/sidevoice_voice.js";
export { createWebAudioIo } from "./web-audio-io.js";
export type { AudioIo, AudioIoSink, WebAudioIoOptions } from "./web-audio-io.js";
export type {
  VoiceAudio,
  VoiceEndOfTurn,
  VoiceLoadedModels,
  VoiceModels,
  VoiceSpeaker,
  VoiceTranscriber,
  VoiceVad,
  VoiceVadFrame,
} from "./voice-models.js";

/** One voice call (`onEvent`, `start`, `stop`, `setConfig`, `setModels`, `roomEvent`, `setOnline`, `mute`,
 *  `cancelInput`). */
export type VoiceCall = WasmVoiceCall;

export interface VoiceCallOptions extends WebAudioIoOptions {
  /** The microphone and speaker to use instead of the browser's (`createWebAudioIo`). */
  io?: AudioIo;
}

export declare const VoiceCall: {
  /**
   * A call on the page's `models` and the browser's microphone and speaker, or `options.io`, set up with `config`
   * (the configuration as JSON; a malformed one throws). It does nothing until `start()`.
   */
  create(models: VoiceModels, config: object, options?: VoiceCallOptions): VoiceCall;
};
