// The voice seam a page drives a call through: `VoiceHost`. Implemented by `createVoiceHost` here (the call in the
// page, over the WebAssembly build and the page's models) and by the Sidevoice desktop app
// (`window.__sidevoiceDesktop.host.voice`, the call run natively). A page uses nothing else of either:
//
//   const voice = window.__sidevoiceDesktop?.host?.voice ?? createVoiceHost(source);
//
// where `source` is the page's `VoiceModelSource`: the models that fill a call's slots, one slot at a time (the page's,
// wired into `voice-models.d.ts`'s interfaces). Which models there are, and how a setting names one, are the page's:
// the host reads a slot's language, voice and speed, and hands the rest of it to `source` as it is.
//
//   const call = voice.start({ input: { stt, endOfTurn: { kind: "silence", ms: 1500 } } }); // a transcription try
//   const call = voice.start({ output: { tts, voice, speed } });                               // listen only: no mic
//   await call.ready;
//
// Each call emits its own turns and saying outcomes (`voice-events.d.ts`).

import type { WebAudioIoOptions } from "./web-audio-io.js";
import type { SharedWebAudioIo } from "./shared-web-audio-io.js";
import type { VoiceEndOfTurn, VoiceSpeaker, VoiceTranscriber, VoiceVad } from "./voice-models.js";
import type {
  VoiceCallState,
  VoiceSaying,
  VoiceSayOptions,
  VoiceTake,
  VoiceTurnEvent,
} from "./voice-events.js";

/** A model as the page's `source` names it (`{catalog, model}`, say): the host hands it to `source` as it is, and two
 *  choices with the same fields are the same model. */
export interface VoiceModelChoice {
  [choice: string]: unknown;
}

/** What ends a turn: a pause of `ms` after the detector says speech stopped (the patience's when absent; loads no
 *  end-of-turn model), or the page's end-of-turn classifier (`model`, as the source names it; the source's own choice
 *  when absent) at each pause. */
export type VoiceEndOfTurnSettings =
  | { kind: "silence"; ms?: number }
  | { kind: "smart"; model?: VoiceModelChoice | null };

/** The input half: the microphone, the detector, the transcriber and, for `smart`, the end-of-turn classifier. */
export interface VoiceInputSettings {
  /** The transcriber, as the source names it. */
  stt: VoiceModelChoice & {
    /** A BCP 47 tag; null or absent to detect it. Not part of the model's choice. */
    language?: string | null;
  };
  /** `{kind: "silence"}` when absent. */
  endOfTurn?: VoiceEndOfTurnSettings;
  patience?: "fast" | "normal" | "calm";
  /** How loud speech must be to count, as a volume from 0 to 1 (Pipecat's: the last 400 ms's loudness, −110 LUFS to −10
   *  LUFS), while nothing plays and while something does (the bar a barge-in must clear). `{quiet: 0.5, playing: 0.65}`. */
  listening_bar?: { quiet: number; playing: number };
  /** Qualified speech, in seconds above 0 and up to 60, before sounding agent audio is cut (0.6). */
  barge_in_min_speech?: number;
  /** The sliding window that speech falls in, as a multiple of `barge_in_min_speech`, from 1 to 10 (2.5). */
  barge_in_window?: number;
  /** The detector's confidence from which a window is speech, above 0 and up to 1 (0.6). */
  vad_confidence?: number;
  /** How long speech must go on before it starts a turn in silence, in milliseconds, 0 to 5000 (400). */
  speech_start_ms?: number;
  /** The audio kept before a turn opens, in milliseconds, 0 to 5000 (1000). */
  pre_roll_ms?: number;
}

/** The output half: the speaker model and the playback. */
export interface VoiceOutputSettings {
  /** The speaker model, as the source names it. */
  tts: VoiceModelChoice;
  /** One of the model's voices; null or absent for the speaker's own choice. */
  voice?: string | null;
  speed?: number;
  /** How long the speaker fades out when playback is cut, in milliseconds, 0 to 1000 (30). */
  fade_ms?: number;
  /** The language of what is said when `say` gives none; absent: the input's. */
  language?: string | null;
}

/** The person's choices for one call: each half optional, at least one. A half that is absent is neither loaded nor
 *  opened: without `input` there is no microphone (a blocked one cannot fail the call), no detector and no transcriber;
 *  without `output` no speaker model and no playback. Barge-in needs both. */
export interface VoiceSettings {
  input?: VoiceInputSettings | null;
  output?: VoiceOutputSettings | null;
  /** How long after a turn ends what is to be said waits, in milliseconds, 0 to 60000 (1000). */
  audio_grace_ms?: number;
  /** How long a model no call holds stays loaded, in minutes (10 by default; 0: let go at once). */
  idle_unload_minutes?: number;
}

/** A change of a running call's settings: a half that is absent stays as it is, `null` removes it, and settings
 *  replace it (or add it). */
export interface VoiceSettingsChange {
  input?: VoiceInputSettings | null;
  output?: VoiceOutputSettings | null;
  audio_grace_ms?: number;
  idle_unload_minutes?: number;
}

/** What a failed call rejects with, and what `onError` hears: rely on `code`. */
export interface VoiceHostError {
  /** `microphone-denied`, `microphone-unavailable`, `microphone-lost`, `speaker-unavailable`,
   *  `audio-device-unavailable`, `echo-cancellation-unavailable`, `stopped`, `end-of-turn-missing`, and the codes of the
   *  page's models and of its source's refusals of a choice, as they give them. */
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

/** The owner of the shared devices and of the models its calls load. */
export interface VoiceHost {
  /** Starts a call with the halves of `settings`: it loads the models they use (each slot on its own, all at once; one
   *  another call holds is not loaded again), opens the speaker and, with an input half, the microphone, and listens.
   *  With `take`, the call keeps what the microphone gives from the moment it listens until that listening ends, and
   *  hands it to `onTake` then; never otherwise. Answers at once; `ready` says when it works. Malformed settings, or
   *  none of the halves, throw. */
  start(settings: VoiceSettings, options?: { take?: boolean }): VoiceCallHost;
  /** Loads the models `settings` use into the host, ahead of a call: the slots a `start` of them would ask for (an
   *  absent half loads none of its slots, `silence` no end-of-turn model), each once for a choice already loaded or
   *  loading. It opens no microphone and no speaker. The models are held as a stopped call's are: a call that starts
   *  with the same choices, now or later, takes them without loading, holds them while it runs, and they are let go
   *  `idle_unload_minutes` (10 by default; the settings' own) after the last holder, counted from the end of the load
   *  (from the call's stop, for one a call held). Resolves once every slot is loaded. Rejects `VoiceHostError` with the
   *  code a slot's load refused (what else loaded stays; the refused slot is forgotten, so the next ask loads it
   *  again), or a `TypeError` for malformed settings or none of the halves. */
  preload(settings: VoiceSettings): Promise<void>;
  /** Acknowledged next-frame switch; null supplies silence to every call. */
  routeInput(target: VoiceCallHost | null): Promise<void>;
  audioDevices(): Promise<VoiceAudioDevices>;
  setAudioDevices(choice: { input?: string | null; output?: string | null }): Promise<VoiceAudioDevices>;
  onAudioDevices(listener: (devices: VoiceAudioDevices) => void): () => void;
}

/** One call: its state, events, and saying handles are independent of every other call. */
export interface VoiceCallHost {
  readonly id: string;
  /** Resolves once the call works: at its first state whose `listening` is not `idle` (`off` for a call without an
   *  input half). Rejects `VoiceHostError`: a model that cannot load, the devices refused, `smart` without an
   *  end-of-turn classifier (`end-of-turn-missing`), or `{code: "stopped"}` when `stop()` comes first. */
  readonly ready: Promise<void>;
  /** Changes the call while it runs, without restarting it: other models, another end of turn, another voice, a half
   *  added or removed. A half whose model choices stay keeps what it loaded; one with other choices loads them while
   *  the call goes on as it was. Each change takes effect at its safe point, never cutting what is playing: the input
   *  half's at the next turn (no turn open), the output half's at the next utterance (what is being said ends in the
   *  old voice; what is asked meanwhile waits for the new one). Adding `input` (a microphone appeared, the permission
   *  was granted) opens the microphone then; removing it closes it. A model that cannot load is an `onError` with its
   *  code, and that half stays as it was. Resolves once the change is accepted, not applied; malformed settings, or
   *  none of the halves left, reject and change nothing.
   *
   *  A call whose microphone is lost (`microphone-lost`, `microphone-denied`, ... through `onError`) goes on with its
   *  output half only, its state's `listening` `off`; `update({input})` adds the microphone back. A call without an
   *  output half has nothing left then, and stops. */
  update(change: VoiceSettingsChange): Promise<void>;
  /** Ends the call: the turn not told yet is cancelled, what is being said or waits to be ends (`stopped`), its audio
   *  lane is released, and the models it held are let go `idle_unload_minutes` later unless another call holds them.
   *  After a start that asked for a take, it resolves once the take was handed to `onTake`. A second call answers the
   *  first's promise. */
  stop(): Promise<void>;
  /** Says `text` after whatever is being said, once nothing of the person's holds it back. The handle tells its steps
   *  and how it ended, and cancels it. After `stop()` its outcome is `not-played` (`stopped`); without an output half,
   *  `not-played` (`failed`, `output-missing`). */
  say(text: string, options?: VoiceSayOptions): VoiceSaying;
  /** Mutes or unmutes the microphone; muting feeds silence and closes an open turn normally. */
  mute(muted: boolean): void;
  /** Cancels what the person said that is not told yet. */
  cancelInput(): void;
  /** Closes the open turn now with what was captured so far: it is transcribed and told like any turn (`finished` with
   *  the words, through `onTurn`). Nothing happens when no turn is open, so a second call is harmless. It does not touch
   *  playback, mute, other calls or how later turns end; the next utterance opens a fresh turn. Resolves once the turn
   *  is closed, not transcribed: the words arrive later as its `finished`, after the transcription and the merge window
   *  (a turn that opens within that window joins it, and this one is then `cancelled` with `merged`, its words carried
   *  by the later `finished`). Mute is not a substitute: it is ordinary silence and does not end a turn. After `stop()`
   *  it resolves at once. */
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
  /** The take of a start that asked for one (`start(settings, {take: true})`), once per stretch of listening, as it
   *  ends: before `stop()` resolves, before the error that ended it. Handed over: the call keeps no copy. */
  onTake(listener: (take: VoiceTake) => void): () => void;
}

/** A loaded voice activity detector, from which each call gets its own stream. */
export interface VoiceVadModel {
  createVad(): VoiceVad | Promise<VoiceVad>;
}

/** The page's side of `createVoiceHost`: the models that fill a call's slots, one slot at a time. Each method loads the
 *  model and answers it (or a promise of it), or rejects `{code}` for a choice it cannot fill; the host asks only for
 *  what a call's halves use, and only when no call holds that slot's model for the same choice already. */
export interface VoiceModelSource {
  /** The voice activity detector, the page's own choice. */
  vad(): VoiceVadModel | Promise<VoiceVadModel>;
  /** The transcriber `choice` names: the input settings' `stt` without its `language`. */
  transcriber(choice: VoiceModelChoice): VoiceTranscriber | Promise<VoiceTranscriber>;
  /** The speaker `choice` names: the output settings' `tts`. */
  speaker(choice: VoiceModelChoice): VoiceSpeaker | Promise<VoiceSpeaker>;
  /** The end-of-turn classifier `choice` names (null: the source's own), for `{kind: "smart"}`. Absent: `smart` is
   *  refused with `end-of-turn-missing`. */
  endOfTurn?(choice: VoiceModelChoice | null): VoiceEndOfTurn | Promise<VoiceEndOfTurn>;
}

export interface VoiceHostOptions extends WebAudioIoOptions {
  /** A shared audio factory with attach and acknowledged routeInput. */
  io?: SharedWebAudioIo;
}

/** The voice seam over this package's call, on the page's `source` of models. */
export declare function createVoiceHost(source: VoiceModelSource, options?: VoiceHostOptions): VoiceHost;
