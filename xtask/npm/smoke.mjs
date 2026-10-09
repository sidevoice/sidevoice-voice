// `cargo xtask npm-smoke` (xtask/src/npm.rs) runs this from a directory where @sidevoice/voice was installed from
// its tarball, as a consumer installs it: the package as Node resolves it, its wasm (the file named by the first
// argument, relative to the entry point) read from node_modules, and a call started on a plain-object engine and a
// plain-object microphone and speaker, with the models named by the second argument (vad,stt,tts); then the voice seam,
// `createVoiceHost`, on the same engine. It prints what it saw; the checks are xtask's.
import { readFileSync } from "node:fs";
import { createVoiceHost, createWebAudioIo, initSync, VoiceCall } from "@sidevoice/voice";

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
  async models() {
    return [
      { id: "silero-vad", capabilities: ["vad"], builds: [] },
      { id: stt, capabilities: ["stt"], builds: [{ id: stt + "/smoke", available: true }] },
      { id: tts, capabilities: ["tts"], builds: [] },
    ];
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
const callLoaded = loaded.slice();
const callState = state;

// The voice seam on the same engine: refusals by code, a start that resolves once listening and at once after, a
// stop, events by kind, and provider keys in the store it was given.
const code = (promise) => promise.then(() => "resolved", (error) => error.code);
const stored = new Map();
const keys = { get: (p) => stored.get(p) ?? null, set: (p, k) => (k == null ? stored.delete(p) : stored.set(p, k)) };
const voice = createVoiceHost(engine, { io, keys });
const states = [];
voice.onState((s) => states.push(s.listening));
const host = { missing: await code(voice.start()) };
host.unknown = await code(voice.setSettings({ stt: { model: "nope" }, tts: { model: tts } }));
host.wrongTask = await code(voice.setSettings({ stt: { model: tts }, tts: { model: tts } }));
host.buildUnfit = await code(voice.setSettings({ stt: { model: stt, build: "nope" }, tts: { model: tts } }));
host.smartTurn = await code(voice.setSettings({ stt: { model: stt }, tts: { model: tts }, end_of_turn: "smart-turn" }));
await voice.setSettings({ stt: { model: stt, build: stt + "/smoke", language: "es" }, tts: { model: tts }, patience: "fast" });
host.started = await code(voice.start());
host.again = await code(voice.start());
await voice.stop();
await new Promise((resolve) => setTimeout(resolve, 50));
host.states = states;
await voice.setProviderKey("openai", "sk-smoke");
host.keys = [await voice.hasProviderKey("openai"), stored.get("openai")];
await voice.setProviderKey("openai", null);
host.keys.push(await voice.hasProviderKey("openai"));
host.seam = Object.keys(voice).sort();

console.log(
  JSON.stringify({ wasm: wasm.pathname, loaded: callLoaded, ioStarted, state: callState, errors, webAudioIo: typeof createWebAudioIo, host }),
);
