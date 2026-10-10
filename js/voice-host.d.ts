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
// The call knows nothing of the room: it tells the page the person's turns under its own ids, and says what the page
// asks it to through a handle that tells how it went (`voice-events.d.ts`). The page translates both ways.

import type { AudioIo, WebAudioIoOptions } from "./web-audio-io.js";
import type { VoiceModels } from "./voice-models.js";
import type {
  VoiceCallState,
  VoiceSaying,
  VoiceSayOptions,
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
  /** How long the models stay in memory with the call stopped, in minutes (10 by default; 0: they leave as it stops).
   *  The next `start` loads them again. */
  idle_unload_minutes?: number;
}

/** What a failed call rejects with, and what `onError` hears: rely on `code`. The desktop app adds its `key`. */
export interface VoiceHostError {
  /** `microphone-denied`, `microphone-unavailable`, `speaker-unavailable`, `audio-device-unavailable`,
   *  `settings-missing`, `stopped`, `end-of-turn-missing`, and the codes of the page's models and of its source's
   *  refusals of settings, as they give them. */
  code: string;
  message?: string;
  key?: string;
}

/** The voice seam. Every event of a call reaches the listeners subscribed when it is emitted, in the order the call
 *  emitted it, across kinds; nothing is buffered, so subscribe before `start`. Each `on…` returns `stop`. */
export interface VoiceHost {
  /** Sets the person's choices; the first creates the call. Rejects `VoiceHostError`. */
  setSettings(settings: VoiceSettings): Promise<void>;
  /** Loads the models, opens the microphone and the speaker, listens. `smart-turn` without an end-of-turn model
   *  rejects `{code: "end-of-turn-missing"}`. Resolves once the microphone and the speaker work, at the first
   *  state whose `listening` is not `idle` (at once if the call listens already; a second `start` while one is pending
   *  settles with it). Rejects `VoiceHostError`; with `{code: "stopped"}` when `stop()` comes first. */
  start(): Promise<void>;
  /** Stops listening and speaking: the turn not told yet is cancelled, and what is being said or waits to be ends
   *  (`stopped`). The models stay loaded. Safe at any time; a `start` after it, awaited or not, starts the call
   *  again. */
  stop(): Promise<void>;
  /** Says `text` after whatever is being said, once nothing of the person's holds it back. The handle tells its steps
   *  and how it ended, and cancels it. Before `setSettings`, or with the call stopped, its outcome is `not-played`
   *  (`stopped`). */
  say(text: string, options?: VoiceSayOptions): VoiceSaying;
  /** Mutes or unmutes the microphone; muting ends the open turn with what was said. Kept from the first call, even
   *  before `setSettings`. */
  mute(muted: boolean): void;
  /** Cancels what the person said that is not told yet. */
  cancelInput(): void;
  /** For each `turn_id`: `started`, then exactly one `finished` (the words) or `cancelled`. */
  onTurn(listener: (turn: VoiceTurnEvent) => void): () => void;
  /** The state, each time it changes (the first once the call starts). */
  onState(listener: (state: VoiceCallState) => void): () => void;
  /** The microphone's level, 0 to 1, once per detector window (about 30 a second). */
  onLevel(listener: (level: number) => void): () => void;
  onError(listener: (error: VoiceHostError) => void): () => void;
}

/** The page's side of `createVoiceHost`: which models fill the call's slots. */
export interface VoiceModelSource {
  /** The models for `settings` (the voice activity detector, the transcriber, the speaker, and an end-of-turn
   *  classifier for `smart-turn`), not loaded yet: the call loads them as it starts. Rejects `{code}` for settings it
   *  cannot fill. Asked again only when what chooses models changes: a slot's fields other than its language, voice
   *  and speed, or the end of turn. */
  models(settings: VoiceSettings): VoiceModels | Promise<VoiceModels>;
}

export interface VoiceHostOptions extends WebAudioIoOptions {
  /** The microphone and speaker instead of the browser's (`createWebAudioIo`). */
  io?: AudioIo;
}

/** The voice seam over this package's call, on the page's `source` of models. */
export declare function createVoiceHost(source: VoiceModelSource, options?: VoiceHostOptions): VoiceHost;
