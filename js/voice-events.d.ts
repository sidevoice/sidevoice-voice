// What a call tells the page, and what becomes of what it is asked to say: the JSON of the crate's VoiceEvent,
// SayEvent and SayOutcome (src/event.rs, src/say.rs). The call knows nothing of the room; the page translates.

/** A step of a turn of the person's speech, under the call's own `turn_id` (the same in every step). */
export type VoiceTurnEvent =
  | { phase: "started"; turn_id: string; /** Unix milliseconds. */ started_at: number }
  | {
      phase: "finished";
      turn_id: string;
      /** What the person said. */
      text: string;
      language?: string;
      /** Unix milliseconds: when the person started (the first of the turns it joined) and when the turn ended. */
      started_at: number;
      ended_at: number;
      /** Whether it joined earlier turns, which were cancelled with `merged`. */
      merged: boolean;
      timings: { audio_ms: number; endpoint_silence_ms: number; recognition_ms: number };
    }
  | { phase: "cancelled"; turn_id: string; /** Joined into the next turn. */ merged: boolean };

/** Where the call is. */
export interface VoiceCallState {
  /**
   * The microphone: `idle` (not started), `muted` (by the person), `speaking` while the person speaks in an open turn,
   * `listening` otherwise: no turn open, or a pause in the open one (a silence longer than the gaps between words),
   * which goes on until its end is decided.
   */
  listening: "idle" | "muted" | "listening" | "speaking";
  /** How many of the person's turns wait for, or are in, transcription. */
  recognising: number;
  playback: "idle" | "synthesizing" | "playing";
  /**
   * Whether the microphone gives audio: `muted` while its device gives none (a muted track: another app or the system
   * holds it, or the device stopped delivering), which is not the person's mute (`listening: "muted"`).
   */
  microphone: "live" | "muted";
}

/** Every event a call's `onEvent` hears. */
export type VoiceCallEvent =
  | { type: "turn"; data: VoiceTurnEvent }
  | { type: "state"; data: VoiceCallState }
  | { type: "level"; /** The microphone's smoothed level, 0 to 1. */ data: number }
  | { type: "error"; data: { code: string } }
  | { type: "take"; data: VoiceTake };

/**
 * What the microphone gave a call from the moment it listened to the moment that listening ended, as the call heard it,
 * and where the person's turns fell in it (src/take.rs): kept only when its start asked for it (`start({take: true})`),
 * held in memory, and handed over once as the listening ends (a stop, a failure, a restart on other models), before the
 * error that ended it and before the state that says the call stopped. The call keeps no copy. Every time in it is in
 * milliseconds from its first sample, at the call's own positions in the audio. It holds at most two minutes of audio.
 */
export interface VoiceTake {
  /** 16000: the call's own rate. */
  sampleRate: number;
  /** Mono samples from -1 to 1, in order, after echo cancellation; silence while the person had the microphone
   *  muted. */
  samples: Float32Array;
  /** Unix milliseconds of its first sample, and of its end. */
  started_at: number;
  ended_at: number;
  /** The turns the call opened in it, in order. */
  turns: VoiceTakeTurn[];
}

/** One of the person's turns within a take, under the `turn_id` its `VoiceTurnEvent`s carry. */
export interface VoiceTakeTurn {
  turn_id: string;
  /** Where the call heard the speech the turn opened at. */
  speech_ms: number;
  /** Where its speech paused (a silence longer than the gaps between words) and where it came back, before its end. */
  pauses: { from_ms: number; to_ms: number }[];
  /** Where the call decided the turn had ended (its transcription starting); null when the take ended first, or the
   *  turn was cancelled while open. */
  end_ms: number | null;
}

/** How to say something. */
export interface VoiceSayOptions {
  /** The language the text is written in, a BCP 47 tag; the configuration's when absent. */
  language?: string;
}

/** Why something said stopped short, or never sounded. */
export type VoiceStopReason =
  | { reason: "cancelled" }
  | { reason: "barge-in" }
  | { reason: "stopped" }
  | { reason: "failed"; /** A stable code, also told as an error. */ code: string };

/** How something said ended. `heard_chars` counts Unicode scalar values, at chunk boundaries. */
export type VoiceSayOutcome =
  | { status: "heard" }
  | ({ status: "heard-up-to"; heard_chars: number } & VoiceStopReason)
  | ({ status: "not-played" } & VoiceStopReason);

/** A step of something being said, in order; `done` is the last. */
export type VoiceSayEvent =
  | { type: "playing" }
  | {
      type: "progress";
      /** The characters of the chunk sounding now, `[from, to)`, or `null` between chunks. */
      sounding: [number, number] | null;
      heard_chars: number;
    }
  | { type: "done"; outcome: VoiceSayOutcome };

/** Something the call is saying: the call's `id` for it, `cancel()`, its steps, and how it ended. */
export interface VoiceSaying {
  readonly id: string;
  /** Cancels it: the part not yet heard is dropped, and its outcome says `cancelled`. */
  cancel(): void;
  /** Hears each of its steps from now on. */
  onEvent(listener: (event: VoiceSayEvent) => void): void;
  /** How it ended, once it has. */
  readonly outcome: Promise<VoiceSayOutcome>;
}
