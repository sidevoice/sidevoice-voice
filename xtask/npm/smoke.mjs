// `cargo xtask npm-smoke` (xtask/src/npm.rs) runs this from a directory where @sidevoice/voice was installed from
// its tarball, as a consumer installs it: the package as Node resolves it, its wasm (the file named by the first
// argument, relative to the entry point) read from node_modules, and a call started on a plain-object engine and a
// plain-object microphone and speaker, with the models named by the second argument (vad,stt,tts). It prints what it
// saw; the checks are xtask's.
import { readFileSync } from "node:fs";
import { createWebAudioIo, initSync, VoiceCall } from "@sidevoice/voice";

const wasm = new URL(process.argv[2], import.meta.resolve("@sidevoice/voice"));
initSync({ module: readFileSync(wasm) });
const [vad, stt, tts] = process.argv[3].split(",");

// Every model the fake engine loads has the three capabilities, shaped as the engine's WebEngine hands them out.
const loaded = [];
const model = {
  asVad: () => ({
    stream: async () => ({ sampleRate: 16000, accept: async () => ({ frames: [] }), reset: async () => {} }),
  }),
  asStt: () => ({ transcribe: async () => "" }),
  asTts: () => ({
    voices: async () => [{ id: "smoke-voice" }],
    speak: async () => ({ samples: new Float32Array(160), sampleRate: 16000 }),
  }),
};
const engine = {
  async load(id) {
    loaded.push(id);
    return model;
  },
};

let ioStarted = false;
const io = {
  start(sink) {
    ioStarted = true;
    sink.captured(new Float32Array(160));
  },
  play() {},
  stopPlayback() {},
  stop() {},
};

const config = { vad: { model: vad }, stt: { model: stt }, tts: { model: tts } };
const call = VoiceCall.create(engine, config, { io });
let state = null;
const errors = [];
call.onEvent((event) => {
  if (event.type === "state") state = event.data;
  if (event.type === "error") errors.push(event.data.code);
});
call.start();
for (let waited = 0; state?.listening !== "listening" && errors.length === 0 && waited < 5000; waited += 10) {
  await new Promise((resolve) => setTimeout(resolve, 10));
}
call.stop();
console.log(
  JSON.stringify({ wasm: wasm.pathname, loaded, ioStarted, state, errors, webAudioIo: typeof createWebAudioIo }),
);
