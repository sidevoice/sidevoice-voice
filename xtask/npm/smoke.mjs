// `cargo xtask npm-smoke` (xtask/src/npm.rs) runs this from a directory where @sidevoice/voice was installed from
// its tarball, as a consumer installs it: the package as Node resolves it, its wasm (the file named by the first
// argument, relative to the entry point) read from node_modules, and a call started on plain-object models (the
// interfaces of js/voice-models.d.ts) and a plain-object microphone and speaker. It prints what it saw; the checks
// are xtask's.
import { readFileSync } from "node:fs";
import { createVoiceHost, createWebAudioIo, initSync, VoiceCall } from "@sidevoice/voice";

const wasm = new URL(process.argv[2], import.meta.resolve("@sidevoice/voice"));
initSync({ module: readFileSync(wasm) });

// The page's models, as the call loads them: each load records the slots it filled.
const loaded = [];
const models = {
  async load() {
    loaded.push("vad", "transcriber", "speaker");
    return {
      vad: { accept: async () => [], reset: async () => {} },
      transcriber: { transcribe: async () => "" },
      speaker: { speak: async () => ({ samples: new Float32Array(160), sampleRate: 16000 }) },
    };
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

// Models dropped as the call stops, so no timer of theirs keeps Node running after the last check.
const config = { language: "en", idle_unload_minutes: 0 };
const call = VoiceCall.create(models, config, { io });
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

// The voice seam on a page's source of the same models: refusals by code, a start that resolves once listening and
// at once after, the models loaded again after a stop with 0 idle minutes, smart-turn refused without an end-of-turn
// model, and provider keys in the store it was given.
const code = (promise) => promise.then(() => "resolved", (error) => error.code);
const source = {
  catalogue: async () => [{ id: "smoke-stt", capabilities: ["stt"] }, { id: "smoke-tts", capabilities: ["tts"] }],
  models: async (settings) => {
    if (settings.stt.model !== "smoke-stt") throw Object.assign(new Error("unknown"), { code: "model-unknown" });
    return models;
  },
};
const stored = new Map();
const keys = { get: (p) => stored.get(p) ?? null, set: (p, k) => (k == null ? stored.delete(p) : stored.set(p, k)) };
const voice = createVoiceHost(source, { io, keys });
const states = [];
voice.onState((s) => states.push(s.listening));
const host = { missing: await code(voice.start()) };
host.unknown = await code(voice.setSettings({ stt: { model: "nope" }, tts: { model: "smoke-tts" } }));
await voice.setSettings({
  stt: { model: "smoke-stt", language: "es" }, tts: { model: "smoke-tts" }, patience: "fast", idle_unload_minutes: 0,
});
host.started = await code(voice.start());
host.again = await code(voice.start());
await voice.stop();
await new Promise((resolve) => setTimeout(resolve, 50));
const loadsBefore = loaded.length;
await voice.start();
host.reloaded = (loaded.length - loadsBefore) / 3;
await voice.stop();
await new Promise((resolve) => setTimeout(resolve, 50));
await voice.setSettings({
  stt: { model: "smoke-stt" }, tts: { model: "smoke-tts" }, end_of_turn: "smart-turn", idle_unload_minutes: 0,
});
host.smartMissing = await code(voice.start());
await new Promise((resolve) => setTimeout(resolve, 50));
host.states = states;
await voice.setProviderKey("openai", "sk-smoke");
host.keys = [await voice.hasProviderKey("openai"), stored.get("openai")];
await voice.setProviderKey("openai", null);
host.keys.push(await voice.hasProviderKey("openai"));
host.seam = Object.keys(voice).sort();
host.catalogue = (await voice.models()).length;
console.log(
  JSON.stringify({
    wasm: wasm.pathname, loaded: callLoaded, ioStarted, state: callState, errors, webAudioIo: typeof createWebAudioIo,
    host,
  }),
);
