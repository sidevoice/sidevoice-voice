import type { AudioIo, WebAudioIoOptions } from "./web-audio-io.js";
import type { VoiceInputModels, VoiceOutputModels } from "./voice-models.js";
import type { VoiceCallEvent, VoiceSaying, VoiceSayOptions } from "./voice-events.js";

export { default, initSync, IoSink } from "../dist/sidevoice_voice.js";
export type { InitInput, InitOutput, SyncInitInput } from "../dist/sidevoice_voice.js";
export { createWebAudioIo } from "./web-audio-io.js";
export { createSharedWebAudioIo } from "./shared-web-audio-io.js";
export type { SharedWebAudioIo, InputPort } from "./shared-web-audio-io.js";
export type { AudioIo, AudioIoSink, WebAudioIoOptions } from "./web-audio-io.js";
export type {
  VoiceAudio,
  VoiceEndOfTurn,
  VoiceInputModels,
  VoiceOutputModels,
  VoiceSpeaker,
  VoiceTranscriber,
  VoiceVad,
  VoiceVadFrame,
} from "./voice-models.js";

/** What ends a turn: a pause of `ms` after the detector says speech stopped (the patience's when absent), or the
 *  page's end-of-turn classifier at each pause. */
export type VoiceEndOfTurnConfig = { kind: "silence"; ms?: number } | { kind: "smart" };

/** The input half of a call: the microphone, the detector, the transcriber and, for `smart`, the end-of-turn
 *  classifier. */
export interface VoiceInputSetup {
  models: VoiceInputModels;
  /** A BCP 47 tag given to the transcriber; null or absent to detect it. */
  language?: string | null;
  /** `{kind: "silence"}` by default; only `smart` asks for the end-of-turn classifier. */
  end_of_turn?: VoiceEndOfTurnConfig;
  patience?: "fast" | "normal" | "calm";
  /** How loud speech must be to count, from 0 to 1, while nothing plays and while something does. */
  listening_bar?: { quiet: number; playing: number };
  /** Continuous speech, in seconds, before sounding audio is cut (1.0 by default). */
  barge_in_min_speech?: number;
}

/** The output half of a call: the speaker model and the playback. */
export interface VoiceOutputSetup {
  models: VoiceOutputModels;
  /** The language of what is said when `say` gives none; absent: the input's. */
  language?: string | null;
  /** One of the speaker's voices; null or absent for its own choice. */
  voice?: string | null;
  speed?: number;
}

/** What a call runs: each half optional. A half that is absent (or null) is neither loaded nor opened. */
export interface VoiceSetup {
  input?: VoiceInputSetup | null;
  output?: VoiceOutputSetup | null;
  /** How long after a turn ends what is to be said waits, in milliseconds (1000). */
  audio_grace_ms?: number;
  /** How long the models stay loaded with the call stopped, in minutes (10; 0: dropped as it stops). */
  idle_unload_minutes?: number;
}

/** One voice call: it tells the page its events and says what the page asks it to. It carries no application routing metadata. */
export interface VoiceCall {
  /** Hears every event from now on: turns, state, level, errors. */
  onEvent(listener: (event: VoiceCallEvent) => void): void;
  /** Loads the models its halves use, opens the speaker and, with an input half, the microphone, and listens once they
   *  work (without an input half its state's `listening` is `off`: it only speaks). With `take`, keeps what the
   *  microphone gives from then until that listening ends, told then as a `take` event. Started already: nothing. */
  start(options?: { take?: boolean }): void;
  /** Stops listening and speaking; the models stay loaded. */
  stop(): void;
  /** Another setup, the whole of it. A half whose `models` is the same object keeps what it loaded. A change of
   *  configuration only is in effect at once; one of halves or models restarts a started call once. A malformed one
   *  throws and changes nothing. */
  update(setup: VoiceSetup): void;
  /** Says `text` after whatever is being said; the handle tells how it goes and cancels it. Without an output half its
   *  outcome is `not-played` (`failed`, `output-missing`). */
  say(text: string, options?: VoiceSayOptions): VoiceSaying;
  /** Mutes or unmutes the microphone; muted input counts as silence and closes a turn normally. */
  mute(muted: boolean): void;
  /** Cancels what the person said that is not told yet. */
  cancelInput(): void;
  /** Closes the open turn now with what was captured so far; nothing when none is open. Resolves once the call has closed it. */
  endInput(): Promise<void>;
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
  VoiceTake,
  VoiceTakeTurn,
  VoiceTurnEvent,
} from "./voice-events.js";

export interface VoiceCallOptions extends WebAudioIoOptions {
  /** The microphone and speaker to use instead of the browser's (`createWebAudioIo`). */
  io?: AudioIo;
}

export declare const VoiceCall: {
  /**
   * A call with the halves of `setup` (the page's models, and the configuration as JSON; a malformed one throws), on
   * the browser's microphone and speaker or `options.io`. It loads and opens nothing until `start()`.
   */
  create(setup: VoiceSetup, options?: VoiceCallOptions): VoiceCall;
};

export { createVoiceHost } from "./voice-host.js";
export type {
  VoiceAudioDevice,
  VoiceAudioDevices,
  VoiceHost,
  VoiceCallHost,
  VoiceHostError,
  VoiceHostOptions,
  VoiceEndOfTurnSettings,
  VoiceInputSettings,
  VoiceModelChoice,
  VoiceModelSource,
  VoiceOutputSettings,
  VoiceSettings,
  VoiceVadModel,
} from "./voice-host.js";
