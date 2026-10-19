// The models a call runs, as the page supplies them: JavaScript objects with these methods, which the call reaches
// through wasm-bindgen's structural imports. The same interfaces as the Rust traits of `src/models.rs`. Which model
// fills each slot, and where it runs, is the page's (sidevoice-engine's models, say): the call names none. A method may
// answer a value or a promise of it; a failure is a throw or a rejection with a stable `code`.

/** What a voice activity detector knows after one of its windows. */
export interface VoiceVadFrame {
  /** The sample after the window's last, counted from the stream's start or its last `reset()`. */
  end: number;
  /** Whether speech is going on after it: confirmed, and not yet over. Speech starts and ends where this changes. */
  speech: boolean;
  /** The model's probability of speech for the window, when it gives one. */
  probability?: number;
}

/** A voice activity detector's stream over 16 kHz mono audio. */
export interface VoiceVad {
  /** Takes the next samples, of any length: one frame per whole window they completed, in order. */
  accept(samples: Float32Array): VoiceVadFrame[] | Promise<VoiceVadFrame[]>;
  /** Starts over from sample 0, with no speech. */
  reset(): void | Promise<void>;
}

/** Speech to text. */
export interface VoiceTranscriber {
  /** The text of mono `samples` at `sampleRate`, in `language` (a BCP 47 tag; absent: detected). */
  transcribe(samples: Float32Array, sampleRate: number, language?: string): string | Promise<string>;
}

/** Text to speech. */
export interface VoiceSpeaker {
  /** `text` spoken with `voice` (the speaker's default when absent), in `language`, at `speed` (1 its own). */
  speak(
    text: string,
    voice: string | undefined,
    language: string | undefined,
    speed: number,
  ): VoiceAudio | Promise<VoiceAudio>;
}

/** Mono samples and their rate. */
export interface VoiceAudio {
  samples: Float32Array;
  sampleRate: number;
}

/** An end-of-turn classifier, for `end_of_turn: {kind: "smart"}`. */
export interface VoiceEndOfTurn {
  /** The probability, from 0 to 1, that the speaker of mono `samples` at `sampleRate` has finished their turn. */
  endOfTurn(samples: Float32Array, sampleRate: number): number | Promise<number>;
}

/** Where a call's input half gets its models: each slot asked for only when the call needs it (the detector and the
 *  transcriber as the input half starts, the end-of-turn classifier only for `{kind: "smart"}`), each answering the
 *  loaded model or a promise of it. A failure is a throw or a rejection with a stable `code`. */
export interface VoiceInputModels {
  /** A fresh detector stream for this call (the weights behind it may be shared). */
  vad(): VoiceVad | Promise<VoiceVad>;
  transcriber(): VoiceTranscriber | Promise<VoiceTranscriber>;
  /** Absent: `{kind: "smart"}` refuses to start (`end-of-turn-missing`). */
  endOfTurn?(): VoiceEndOfTurn | Promise<VoiceEndOfTurn>;
}

/** Where a call's output half gets its speaker, loaded. */
export interface VoiceOutputModels {
  speaker(): VoiceSpeaker | Promise<VoiceSpeaker>;
}
