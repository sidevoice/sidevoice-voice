// The voice seam a page drives a call through: `VoiceHost`. Implemented by `createVoiceHost` here (the call in the
// page, over the WebAssembly build and the page's models) and by the Sidevoice desktop app
// (`window.__sidevoiceDesktop.host.voice`, the call run natively). A page uses nothing else of either:
//
//   const voice = window.__sidevoiceDesktop?.host?.voice ?? createVoiceHost(source);
//
// where `source` is the page's `VoiceModelSource`: its catalogue, and the models that fill the call's slots for the
// person's settings (sidevoice-engine's, wired into `voice-models.d.ts`'s interfaces, say).
//
// The payloads are the room's messages as sidevoice-voice writes and reads them (its README, "The room's messages").

import type { AudioIo, WebAudioIoOptions } from "./web-audio-io.js";
import type { VoiceModels } from "./voice-models.js";

/** The person's choices. Each host fills the rest: the voice activity detector, the builds, grace, listening bar. */
export interface VoiceSettings {
  stt: {
    model: string;
    /** One of the model's builds that is `available` here; null or absent for the host's choice. */
    build?: string | null;
    /** A BCP 47 tag; null or absent to detect it. */
    language?: string | null;
  };
  tts: {
    model: string;
    build?: string | null;
    /** One of the model's voices; null or absent for its first (for ElevenLabs, the account's first). */
    voice?: string | null;
    speed?: number;
  };
  patience?: "fast" | "normal" | "calm";
  /** `silence` by default; `smart-turn` needs a model with the `end-of-turn` capability. */
  end_of_turn?: "silence" | "smart-turn";
  /** How long the models stay in memory with the call stopped, in minutes (10 by default; 0: they leave as it stops).
   *  The next `start` loads them again. */
  idle_unload_minutes?: number;
}

/** One phase of a turn of the person's speech: `voice-user-turn`'s `data`. */
export interface VoiceUserTurn {
  /** Unique per message: the outbox's id and what the room acknowledges. */
  client_msg_id: string;
  /** The same on every phase of one turn. */
  turn_id: string;
  phase: "started" | "cancelled" | "finished";
  /** The latest room revision the call had seen on a reply when the turn started (0 before any). */
  revision: number;
  /** In `finished`, and in a `cancelled` merged into the next turn. */
  text?: string;
  language?: string;
  /** Emitted while `setOnline(false)`. */
  offline: boolean;
  /** Unix milliseconds. */
  started_at: number;
  ended_at?: number;
  /** `finished`: it joined earlier turns. `cancelled`: it was joined into the next. */
  merged: boolean;
  timings?: { audio_ms: number; endpoint_silence_ms: number; recognition_ms?: number };
}

/** What became of a reply: `voice-playback`'s `data`. Per utterance: `playing` (if it sounds), then exactly one of
 *  `heard`, `interrupted`, `unplayed`, `failed`. */
export interface VoicePlayback {
  client_msg_id: string;
  utterance_id: string;
  status: "playing" | "heard" | "interrupted" | "unplayed" | "failed";
  /** Characters (Unicode scalar values) of the text heard from its start, at chunk boundaries. */
  heard_chars: number;
  /** Why it failed, as a stable code. */
  reason?: string;
  /** Unix milliseconds. */
  at: number;
}

/** A reply the room wants spoken: `voice-reply`'s `data`, as the room sent it. */
export interface VoiceReply {
  utterance_id: string;
  revision: number;
  reply_revision: number;
  thread_id: string;
  history_id: string;
  text: string;
  language?: string | null;
  /** Said again because the person asked: spoken even though its id was seen. */
  replay?: boolean;
}

/** Where the call is. */
export interface VoiceState {
  listening: "idle" | "muted" | "listening" | "speaking";
  /** Turns waiting for, or in, transcription. */
  recognising: number;
  playback: "idle" | "synthesizing" | "playing";
  online: boolean;
}

/** Where the reader of a reply is, in characters of its text. Only for utterances given to `speak`. */
export interface VoiceKaraoke {
  utterance_id: string;
  /** The chunk sounding now, `[from, to)`; null between chunks. */
  sounding: [number, number] | null;
  heard_chars: number;
}

/** What a failed call rejects with, and what `onError` hears: rely on `code`. The desktop app adds its `key`. */
export interface VoiceHostError {
  /** `microphone-denied`, `microphone-unavailable`, `speaker-unavailable`, `audio-device-unavailable`, the models'
   *  (`model-load-failed`, `credential-missing`, …), `settings-missing`, `stopped`, `end-of-turn-missing`, and the
   *  model source's refusals of settings (`model-unknown`, `model-wrong-task`, `model-unfit`, `build-unfit`,
   *  `end-of-turn-unavailable`, …). */
  code: string;
  message?: string;
  key?: string;
}

/** A build of a catalogue model, as `WebEngine.models()` lists it. A remote one has `accelerator: "remote"`, and its
 *  `backend` is its provider's id (`openai`, `elevenlabs`), the one `setProviderKey` takes. */
export interface VoiceBuild {
  id: string;
  backend: string;
  /** What it would run on here (`cpu`, `metal`, `webgpu`, `wasm`, `remote`…), when it runs here. */
  accelerator?: string;
  precision: string;
  downloadBytes: number;
  memoryMb: number;
  /** Whether it runs here; when not, `reasons` say why. */
  available: boolean;
  reasons: { code: string; params: { needs?: number; has?: number } }[];
  /** On disk; for a remote build, whether the host has its provider's key. */
  installed: boolean;
}

/** A model of the catalogue the settings choose from, in the shape `WebEngine.models()` lists, on both hosts. */
export interface VoiceModel {
  id: string;
  family: string;
  /** `stt`, `tts`, `vad`, `end-of-turn`… */
  capabilities: string[];
  parametersM: number;
  languages: string[];
  license: string;
  voices: { id: string; languages: string[]; gender?: "female" | "male" }[];
  installed: boolean;
  builds: VoiceBuild[];
  recommendedBuild?: string;
}

/** The voice seam. Every event of a call reaches the listeners subscribed when it is emitted, in the order the call
 *  emitted it, across kinds; nothing is buffered, so subscribe before `start`. Each `on…` returns `stop`. */
export interface VoiceHost {
  /** Sets the person's choices; the first creates the call. Rejects `VoiceHostError`. */
  setSettings(settings: VoiceSettings): Promise<void>;
  /** Loads the models, opens the microphone and the speaker, listens. `smart-turn` without an end-of-turn model
   *  rejects `{code: "end-of-turn-missing"}`. Resolves at the first
   *  state whose `listening` is not `idle` (at once if the call listens already; a second `start` while one is pending
   *  settles with it). Rejects `VoiceHostError`; with `{code: "stopped"}` when `stop()` comes first. */
  start(): Promise<void>;
  /** Stops listening and speaking: the turn not reported is cancelled, the reply playing interrupted, the queue
   *  dropped. The models stay loaded. Safe at any time. */
  stop(): Promise<void>;
  /** A room `voice-reply`'s `data`. A reply whose `utterance_id` was seen is ignored unless `replay`. */
  speak(reply: VoiceReply): void;
  /** Whether the room is in reach: turns emitted while it is not carry `offline: true`. */
  setOnline(online: boolean): void;
  /** Mutes or unmutes the microphone; muting ends the open turn with what was said. */
  mute(muted: boolean): void;
  /** Cancels what the person said that is not reported yet. */
  cancelInput(): void;
  /** For each `turn_id`: `started`, then exactly one `finished` or `cancelled`. */
  onUserTurn(listener: (turn: VoiceUserTurn) => void): () => void;
  onPlayback(listener: (report: VoicePlayback) => void): () => void;
  /** The state, each time it changes (the first once the call starts). */
  onState(listener: (state: VoiceState) => void): () => void;
  /** The microphone's level, 0 to 1, once per detector window (about 30 a second). */
  onLevel(listener: (level: number) => void): () => void;
  onKaraoke(listener: (karaoke: VoiceKaraoke) => void): () => void;
  onError(listener: (error: VoiceHostError) => void): () => void;
  /** The catalogue the settings choose from (the model source's). */
  models(): Promise<VoiceModel[]>;
  /** Keeps `key` for a remote provider (`openai`, `elevenlabs`), or removes it with `null`. The models ask the host for
   *  it; the page never reads it back. */
  setProviderKey(provider: string, key: string | null): Promise<void>;
  hasProviderKey(provider: string): Promise<boolean>;
}

/** Where `createVoiceHost` keeps provider keys, and where the page's models read them (an engine host's
 *  `credential`, say). */
export interface ProviderKeys {
  get(provider: string): string | null;
  set(provider: string, key: string | null): void;
}

/** The page's side of `createVoiceHost`: which models fill the call's slots, and what the settings choose from. */
export interface VoiceModelSource {
  /** The catalogue `models()` answers. */
  catalogue(): Promise<VoiceModel[]>;
  /** The models for `settings` (the voice activity detector, the transcriber, the speaker, and an end-of-turn
   *  classifier for `smart-turn`), not loaded yet: the call loads them as it starts. Rejects `{code}` for settings it
   *  cannot fill. Asked again only when the models the settings choose change (a stage or the end of turn). */
  models(settings: VoiceSettings): VoiceModels | Promise<VoiceModels>;
}

export interface VoiceHostOptions extends WebAudioIoOptions {
  /** The microphone and speaker instead of the browser's (`createWebAudioIo`). */
  io?: AudioIo;
  /** Where provider keys are kept; `localStorageProviderKeys()` by default. */
  keys?: ProviderKeys;
}

/** The voice seam over this package's call, on the page's `source` of models. */
export declare function createVoiceHost(source: VoiceModelSource, options?: VoiceHostOptions): VoiceHost;

/** Provider keys in the page's `localStorage`, under `sidevoice.provider-key.<provider>`: the web's choice
 *  (sidevoice-core#89 §3), with the warning that any script of the page can read them. */
export declare function localStorageProviderKeys(): ProviderKeys;
