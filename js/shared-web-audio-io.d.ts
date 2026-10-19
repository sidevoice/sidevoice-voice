import type { AudioIo, WebAudioIoOptions } from "./web-audio-io.js";

/** Opaque input target paired with one playback lane. */
export interface InputPort { readonly id: number }

/** One browser microphone and output context, shared by independently running calls. The microphone is open only while
 *  a started lane asks for it (`io.start(sink, {input: true})`, `io.setInput(true)`); a lane that only speaks never asks
 *  for one. A microphone that goes away (and no default one opens in its place) or cannot open is said to the lanes that
 *  asked for it as `inputLost(code)`; their speakers go on. */
export interface SharedWebAudioIo {
  attach(): { io: AudioIo & { dispose(): void }; input: InputPort };
  /** After this resolves, the next captured frame is real only for target; all other lanes receive zero frames. */
  routeInput(target: InputPort | null): Promise<void>;
  setInputDevice(id: string | null): Promise<void>;
  setOutputDevice(id: string | null): Promise<void>;
  readonly outputSelectable: boolean;
}

export declare function createSharedWebAudioIo(options?: WebAudioIoOptions): SharedWebAudioIo;
