// The voice seam a page drives a call through: `VoiceHost`. Implemented by `createVoiceHost` here (the call in the
// page, over the WebAssembly build and the page's models) and by the Sidevoice desktop app
// (`window.__sidevoiceDesktop.host.voice`, the call run natively). A page uses nothing else of either:
//
//   const voice = window.__sidevoiceDesktop?.host?.voice ?? createVoiceHost(source);
//
// where `source` is the page's `VoiceModelSource`: the models that fill the call's slots for the person's settings
// (the page's, wired into `voice-models.d.ts`'s interfaces). Which models there are, and how a setting names one,
// are the page's: the call reads a slot's language, voice and speed, and hands the rest of it to `source` as it is.
//
// Each call emits its own turns and saying outcomes (`voice-events.d.ts`).

import type { WebAudioIoOptions } from "./web-audio-io.js";
import type { SharedWebAudioIo } from "./shared-web-audio-io.js";
import type { VoiceModels } from "./voice-models.js";
import type {
  VoiceCallState,
  VoiceSaying,
  VoiceSayOptions,
  VoiceTake,
  VoiceTurnEvent,
} from "./voice-events.js";

/** The person's choices. A slot's model is named as the page's `source` names it: the call hands everything in a slot
 *  but the fields below to `source.models(settings)`, and reads nothing else of it. */
export interface VoiceSettings {
  stt: {
    /** A BCP 47 tag; null or absent to detect it. */
    language?: string | null;
    /** The model this slot takes, as the page's source names it. */
    [choice: string]: unknown;
  };
  tts: {
    /** One of the model's voices; null or absent for the speaker's own choice. */
    voice?: string | null;
    speed?: number;
    /** The model this slot takes, as the page's source names it. */
    [choice: string]: unknown;
  };
  patience?: "fast" | "normal" | "calm";
  /** `silence` by default; `smart-turn` needs `source` to give an end-of-turn model. */
  end_of_turn?: "silence" | "smart-turn";
  /** How long an unused model choice stays loaded, in minutes (10 by default; 0: release it at once). */
  idle_unload_minutes?: number;
  /** Continuous raised-bar speech, in seconds, before sounding agent audio is cut (default 1.0). */
  barge_in_min_speech?: number;
}

/** What a failed call rejects with, and what `onError` hears: rely on `code`. */
export interface VoiceHostError {
  /** `microphone-denied`, `microphone-unavailable`, `speaker-unavailable`, `audio-device-unavailable`,
   *  `echo-cancellation-unavailable`, `settings-missing`, `stopped`, `end-of-turn-missing`, and the codes of the page's models and of its source's
   *  refusals of settings, as they give them. */
  code: string;
  message?: string;
}

/** An audio device as the platform lists it. `label` may be empty until the page may use the microphone. */
export interface VoiceAudioDevice {
  id: string;
  label: string;
}

/** The audio devices, and which are chosen. */
export interface VoiceAudioDevices {
  inputs: VoiceAudioDevice[];
  outputs: VoiceAudioDevice[];
  /** The chosen microphone's id; null is the system's default (not an entry of `inputs`). */
  input: string | null;
  /** The chosen output's id; null is the system's default. */
  output: string | null;
  /** Whether the output can be chosen here. When not, `outputs` still lists what exists, and playback stays on the
   *  default one. */
  outputSelectable: boolean;
  /** Once, on the list after a chosen device went away: its id. The default one is in use from then on. */
  lost?: { input?: string; output?: string };
}

/** A generic owner of shared devices and model loads. */
export interface VoiceHost {
  createCall(): VoiceCallHost;
  /** Hand a browser stream and running context begun in a user gesture to the next call. */
  activateAudio?(prepared: { context: AudioContext; resumed: Promise<void>; stream: Promise<MediaStream> }): void;
  /** Acknowledged next-frame switch; null supplies silence to every call. */
  routeInput(target: VoiceCallHost | null): Promise<void>;
  audioDevices(): Promise<VoiceAudioDevices>;
  setAudioDevices(choice: { input?: string | null; output?: string | null }): Promise<VoiceAudioDevices>;
  onAudioDevices(listener: (devices: VoiceAudioDevices) => void): () => void;
}

/** A call's state, events, and saying handles are independent of every other call. */
export interface VoiceCallHost {
  readonly id: string;
  dispose(): Promise<void>;

  /** Sets the person's choices; the first creates the call. Rejects `VoiceHostError`. */
  setSettings(settings: VoiceSettings): Promise<void>;
  /** Loads the models, opens the microphone and the speaker, listens. `smart-turn` without an end-of-turn model
   *  rejects `{code: "end-of-turn-missing"}`. Resolves once the microphone and the speaker work, at the first
   *  state whose `listening` is not `idle` (at once if the call listens already; a second `start` while one is pending
   *  settles with it). Rejects `VoiceHostError`; with `{code: "stopped"}` when `stop()` comes first. With `take`, the
   *  call keeps what the microphone gives from the moment it listens until that listening ends, and hands it to
   *  `onTake` then; never otherwise (a call is not recorded). A start while one runs changes nothing, its take
   *  included. */
  start(options?: { take?: boolean }): Promise<void>;
  /** Stops listening and speaking: the turn not told yet is cancelled, and what is being said or waits to be ends
   *  (`stopped`). The models stay loaded. Safe at any time; a `start` after it, awaited or not, starts the call
   *  again. After a start that asked for a take, it resolves once the take was handed to `onTake`. */
  stop(): Promise<void>;
  /** Says `text` after whatever is being said, once nothing of the person's holds it back. The handle tells its steps
   *  and how it ended, and cancels it. Before `setSettings`, or with the call stopped, its outcome is `not-played`
   *  (`stopped`). */
  say(text: string, options?: VoiceSayOptions): VoiceSaying;
  /** Mutes or unmutes the microphone; muting feeds silence and closes an open turn normally. Kept from the first call, even
   *  before `setSettings`. */
  mute(muted: boolean): void;
  /** Cancels what the person said that is not told yet. */
  cancelInput(): void;
  /** Closes the open turn now with what was captured so far: it is transcribed and told like any turn (`finished` with
   *  the words, through `onTurn`). Nothing happens when no turn is open, so a second call is harmless. It does not touch
   *  playback, mute, other calls or how later turns end; the next utterance opens a fresh turn. Resolves once the turn
   *  is closed, not transcribed: the words arrive later as its `finished`, after the transcription and the merge window
   *  (a turn that opens within that window joins it, and this one is then `cancelled` with `merged`, its words carried
   *  by the later `finished`). Mute is not a substitute: it is ordinary silence and does not end a turn. Before `setSettings`, or
   *  with the call stopped, it resolves at once. */
  endInput(): Promise<void>;
  /** For each `turn_id`: `started`, then exactly one `finished` (the words) or `cancelled`. */
  onTurn(listener: (turn: VoiceTurnEvent) => void): () => void;
  /** The state, each time it changes (the first once the call starts). */
  onState(listener: (state: VoiceCallState) => void): () => void;
  /** The microphone's level, 0 to 1, once per detector window (about 30 a second). */
  onLevel(listener: (level: number) => void): () => void;
  /** The rendered speaker's level, 0 to 1, on detector windows (normally 32 ms); exact zero when output ends.
   *  It continues while the microphone is muted. */
  onPlaybackLevel(listener: (level: number) => void): () => void;
  onError(listener: (error: VoiceHostError) => void): () => void;
  /** The take of a start that asked for one (`start({take: true})`), once per stretch of listening, as it ends: before
   *  `stop()` resolves, before the error that ended it, before a restart on other models listens again. Handed over:
   *  the call keeps no copy. */
  onTake(listener: (take: VoiceTake) => void): () => void;
}

/** The page's side of `createVoiceHost`: which models fill the call's slots. */
export interface VoiceModelSource {
  /** A lightweight model loader for `settings`, not an owner of loaded weights. Its `load()` returns the detector
   *  factory, transcriber, speaker, and optional end-of-turn classifier when a call starts. Rejects `{code}` for settings it
   *  cannot fill. Asked again only when what chooses models changes: a slot's fields other than its language, voice
   *  and speed, or the end of turn. */
  models(settings: VoiceSettings): VoiceModels | Promise<VoiceModels>;
}

export interface VoiceHostOptions extends WebAudioIoOptions {
  /** A shared audio factory with attach and acknowledged routeInput. */
  io?: SharedWebAudioIo;
}

/** The voice seam over this package's call, on the page's `source` of models. */
export declare function createVoiceHost(source: VoiceModelSource, options?: VoiceHostOptions): VoiceHost;
