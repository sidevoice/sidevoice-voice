import type { AudioIo, WebAudioIoOptions } from "./web-audio-io.js";
import type { VoiceModels } from "./voice-models.js";
import type { VoiceCallEvent, VoiceSaying, VoiceSayOptions } from "./voice-events.js";

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

/** One voice call: it tells the page its events and says what the page asks it to. It knows nothing of the room. */
export interface VoiceCall {
  /** Hears every event from now on: turns, state, level, errors. */
  onEvent(listener: (event: VoiceCallEvent) => void): void;
  /** Loads the models, opens the microphone and the speaker, and listens once both work. */
  start(): void;
  /** Stops listening and speaking; the models stay loaded. */
  stop(): void;
  /** A new configuration, as JSON, in effect at once. */
  setConfig(config: object): void;
  /** Other models with the configuration they go with, together (a live call restarts once). */
  setModels(models: VoiceModels, config: object): void;
  /** Says `text` after whatever is being said; the handle tells how it goes and cancels it. */
  say(text: string, options?: VoiceSayOptions): VoiceSaying;
  /** Mutes or unmutes the microphone; muting ends the open turn with what was said. */
  mute(muted: boolean): void;
  /** Cancels what the person said that is not told yet. */
  cancelInput(): void;
  /** Ends the call: the microphone and the speaker close, and the models are dropped. */
  free(): void;
}
export type {
  VoiceCallEvent,
  VoiceCallState,
  VoiceSayEvent,
  VoiceSaying,
  VoiceSayOptions,
  VoiceSayOutcome,
  VoiceStopReason,
  VoiceTurnEvent,
} from "./voice-events.js";

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
