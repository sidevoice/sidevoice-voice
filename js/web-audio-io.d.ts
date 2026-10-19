/** Where a microphone and speaker report: an `IoSink` of the call. */
export interface AudioIoSink {
  /** The microphone and the speaker work: the call listens from now. */
  ready(): void;
  /** Captured audio: 16 kHz mono samples, in order, after echo cancellation. */
  captured(samples: Float32Array): void;
  /** Rendered speaker samples, mono at 16 kHz after gain and fading. */
  rendered(samples: Float32Array): void;
  /** The output finished, including its fade. */
  playbackEnded(): void;
  /** The first sample of a chunk reached the speaker. */
  chunkStarted(utterance: string, chunk: number): void;
  /** The last sample of a chunk reached the speaker. */
  chunkPlayed(utterance: string, chunk: number): void;
  /** Whether the microphone's device gives no audio for now (muted by the browser or the system, not by the person). */
  microphoneMuted(muted: boolean): void;
  /** The microphone failed, went away with no other in its place, or could not open, with a stable code: it is closed,
   *  and the speaker goes on. */
  inputLost(code: string): void;
  /** The speaker failed, or the devices could not open, with a stable code; the call stops. */
  failed(code: string): void;
}

/** A microphone and speaker for one call. */
export interface AudioIo {
  /**
   * Starts opening the speaker and, with `input`, the microphone (none is asked for otherwise); reports to `sink`:
   * `ready()` once what was asked for works (the call listens only from then), `inputLost(code)` for a microphone that
   * cannot open, or `failed(code)`. May throw an `Error` with a stable `code` when it cannot even begin.
   */
  start(sink: AudioIoSink, options?: { input?: boolean }): void;
  /** Opens the microphone (`true`) or closes it (`false`) while the speaker goes on; one that cannot open is said as
   *  `inputLost(code)`. Before `start`, or after `stop`, it does nothing. */
  setInput(input: boolean): void;
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
}

/**
 * The browser's microphone (`getUserMedia` with its echo cancellation, at 16 kHz mono, asked for only with `input`) and
 * speaker (Web Audio). A microphone that cannot open or goes away reaches `sink.inputLost` as `microphone-denied`,
 * `microphone-unavailable`, `echo-cancellation-unavailable` or `microphone-lost`, and the speaker goes on; other
 * failures reach `sink.failed` as `audio-capture-failed` or `audio-output-failed`. Its devices can be chosen at any time
 * (`setInputDevice`, `setOutputDevice`); a chosen microphone that goes away falls back to the default one
 * (`onDeviceLost`).
 */
export function createWebAudioIo(options?: WebAudioIoOptions): AudioIo;
