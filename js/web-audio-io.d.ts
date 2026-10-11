/** Where a microphone and speaker report: an `IoSink` of the call. */
export interface AudioIoSink {
  /** The microphone and the speaker work: the call listens from now. */
  ready(): void;
  /** Captured audio: 16 kHz mono samples, in order, after echo cancellation. */
  captured(samples: Float32Array): void;
  /** The first sample of a chunk reached the speaker. */
  chunkStarted(utterance: string, chunk: number): void;
  /** The last sample of a chunk reached the speaker. */
  chunkPlayed(utterance: string, chunk: number): void;
  /** The microphone or the speaker failed, with a stable code; the call stops. */
  failed(code: string): void;
}

/** A microphone and speaker for one call. */
export interface AudioIo {
  /**
   * Starts opening the microphone and the speaker; reports to `sink`: `ready()` once both work (the call listens only
   * from then), or `failed(code)`. May throw an `Error` with a stable `code` when it cannot even begin.
   */
  start(sink: AudioIoSink): void;
  /** Queues a chunk of a reply: mono samples at `sampleRate`, played after whatever is queued. */
  play(utterance: string, chunk: number, samples: Float32Array, sampleRate: number): void;
  /** Stops the speaker at once, with a short fade, and drops every queued chunk. */
  stopPlayback(): void;
  /** Stops capturing and closes the speaker. */
  stop(): void;
  /** Optional: the microphone from now, by id (null: the default); a running one switches without ending. Rejects with
   *  a stable `code`. */
  setInputDevice?(id: string | null): Promise<void>;
  /** Optional: the output from now, by id (null: the default), on what is playing too. Rejects with a stable `code`. */
  setOutputDevice?(id: string | null): Promise<void>;
  /** Optional: whether `setOutputDevice` can choose an output here. */
  readonly outputSelectable?: boolean;
}

export interface WebAudioIoOptions {
  /** The id of an audio input (`MediaDeviceInfo.deviceId`) to capture from; the default microphone otherwise. */
  inputDevice?: string;
  /** Hears that a chosen device went away (`"input"`, its id): the default one is in use from then on. */
  onDeviceLost?: (kind: "input" | "output", id: string) => void;
  /**
   * The id of an audio output (`MediaDeviceInfo.deviceId`) to play on, where the browser lets an `AudioContext`
   * choose; the default output otherwise. A non-default output may escape the browser's echo canceller.
   */
  outputDevice?: string;
  /**
   * Whether the browser cancels the echo of what the call plays out of the microphone (`getUserMedia`'s
   * `echoCancellation`): on unless `false`. Off, the call hears its own replies; for trying what it does without.
   */
  echoCancellation?: boolean;
}

/**
 * The browser's microphone (`getUserMedia` with its echo cancellation, at 16 kHz mono) and speaker (Web Audio).
 * Failures reach `sink.failed` as `microphone-denied`, `microphone-unavailable`, `microphone-lost`,
 * `audio-capture-failed` or `audio-output-failed`. Its devices can be chosen at any time (`setInputDevice`,
 * `setOutputDevice`); a chosen microphone that goes away falls back to the default one (`onDeviceLost`).
 */
export function createWebAudioIo(options?: WebAudioIoOptions): AudioIo;
