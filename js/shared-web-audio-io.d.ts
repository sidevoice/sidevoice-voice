import type { AudioIo, WebAudioIoOptions } from "./web-audio-io.js";

/** Opaque input target paired with one playback lane. */
export interface InputPort { readonly id: number }

/** One browser microphone and output context, shared by independently running calls. */
export interface SharedWebAudioIo {
  activate(prepared: { context: AudioContext; resumed: Promise<void>; stream: Promise<MediaStream> }): void;
  attach(): { io: AudioIo & { dispose(): void }; input: InputPort };
  /** After this resolves, the next captured frame is real only for target; all other lanes receive zero frames. */
  routeInput(target: InputPort | null): Promise<void>;
  setInputDevice(id: string | null): Promise<void>;
  setOutputDevice(id: string | null): Promise<void>;
  readonly outputSelectable: boolean;
}

export declare function createSharedWebAudioIo(options?: WebAudioIoOptions): SharedWebAudioIo;
