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
  listening: "idle" | "muted" | "listening" | "speaking";
  /** How many of the person's turns wait for, or are in, transcription. */
  recognising: number;
  playback: "idle" | "synthesizing" | "playing";
}

/** Every event a call's `onEvent` hears. */
export type VoiceCallEvent =
  | { type: "turn"; data: VoiceTurnEvent }
  | { type: "state"; data: VoiceCallState }
  | { type: "level"; /** The microphone's smoothed level, 0 to 1. */ data: number }
  | { type: "error"; data: { code: string } };

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
