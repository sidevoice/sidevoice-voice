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
// The microphone answers a little later, as a permission prompt does: the call must not listen before it is ready.
let ready = false;
let listenedEarly = false;
let speaker = null;
const io = {
  start(sink) {
    ioStarted = true;
    speaker = sink;
    setTimeout(() => {
      ready = true;
      sink.ready();
      sink.captured(new Float32Array(160));
    }, 50);
  },
  // Plays at once: each chunk starts and ends as it is queued.
  play(utterance, chunk) {
    speaker.chunkStarted(utterance, chunk);
    speaker.chunkPlayed(utterance, chunk);
  },
  stopPlayback() {},
  stop() {},
};

// Models dropped as the call stops, so no timer of theirs keeps Node running after the last check.
const config = { language: "en", idle_unload_minutes: 0 };
const call = VoiceCall.create(models, config, { io });
let state = null;
const errors = [];
call.onEvent((event) => {
  if (event.type === "state") {
    state = event.data;
    if (state.listening !== "idle" && !ready) listenedEarly = true;
  }
  if (event.type === "error") errors.push(event.data.code);
});
call.start();
for (let waited = 0; state?.listening !== "listening" && errors.length === 0 && waited < 5000; waited += 10) {
  await new Promise((resolve) => setTimeout(resolve, 10));
}
// Something said: its handle tells the steps and how it ended.
const saying = call.say("The smoke test speaks this sentence.", { language: "en" });
const steps = [];
saying.onEvent((step) => steps.push(step.type));
const outcome = await Promise.race([saying.outcome, new Promise((resolve) => setTimeout(() => resolve(null), 5000))]);
const said = { id: typeof saying.id, steps, outcome };
call.stop();
const callLoaded = loaded.slice();
const callState = state;

// The voice seam on a page's source of the same models: refusals by code, flags set before the settings kept, a
// start that resolves once listening and at once after, a start straight after a stop that listens, the models
// loaded again after a stop with 0 idle minutes, a live switch to smart-turn with an end-of-turn model, smart-turn
// refused without one, and provider keys in the store it was given.
const code = (promise) => promise.then(() => "resolved", (error) => error.code);
const source = {
  catalogue: async () => [{ id: "smoke-stt", capabilities: ["stt"] }, { id: "smoke-tts", capabilities: ["tts"] }],
  models: async (settings) => {
    if (settings.stt.model !== "smoke-stt") throw Object.assign(new Error("unknown"), { code: "model-unknown" });
    // A build that comes with an end-of-turn model, for smart-turn.
    return settings.stt.build === "ends-turns" ? ending : models;
  },
};
const ending = {
  async load() {
    return { ...(await models.load()), endOfTurn: { endOfTurn: async () => 0.9 } };
  },
};
const stored = new Map();
const keys = { get: (p) => stored.get(p) ?? null, set: (p, k) => (k == null ? stored.delete(p) : stored.set(p, k)) };
const voice = createVoiceHost(source, { io, keys });
const states = [];
const seen = [];
const hostErrors = [];
voice.onState((s) => {
  states.push(s.listening);
  seen.push(s);
});
voice.onError((e) => hostErrors.push(e.code));
const until = async (done) => {
  for (let waited = 0; !done() && waited < 5000; waited += 10) await new Promise((r) => setTimeout(r, 10));
};
const host = { missing: await code(voice.start()) };
// Said before any settings: there is no call, so it is not played.
host.early = (await voice.say("Too early.").outcome).status;
// Muted before any settings: the call they create must start that way.
voice.mute(true);
host.unknown = await code(voice.setSettings({ stt: { model: "nope" }, tts: { model: "smoke-tts" } }));
await voice.setSettings({
  stt: { model: "smoke-stt", language: "es" }, tts: { model: "smoke-tts" }, patience: "fast", idle_unload_minutes: 0,
});
host.started = await code(voice.start());
const first = seen.find((s) => s.listening !== "idle");
host.flags = [first?.listening];
voice.mute(false);
host.again = await code(voice.start());
// Said with the call listening: heard, its handle telling how.
const sayingHost = voice.say("The host says this.", { language: "en" });
host.said = [typeof sayingHost.cancel, (await sayingHost.outcome).status];
// A stop and a start straight after it, both awaited, no pause: the call ends up listening.
await voice.stop();
host.restarted = await code(voice.start());
await new Promise((resolve) => setTimeout(resolve, 100));
host.afterRestart = states.at(-1);
// Stopped with 0 idle minutes, the models leave memory; the next start loads them again.
await voice.stop();
await until(() => states.at(-1) === "idle");
await new Promise((resolve) => setTimeout(resolve, 50));
const loadsBefore = loaded.length;
await voice.start();
host.reloaded = (loaded.length - loadsBefore) / 3;
// Silence → smart-turn on the live call, with models that end turns: it keeps listening.
const errorsBefore = hostErrors.length;
await voice.setSettings({
  stt: { model: "smoke-stt", build: "ends-turns" }, tts: { model: "smoke-tts" }, end_of_turn: "smart-turn",
  idle_unload_minutes: 0,
});
await new Promise((resolve) => setTimeout(resolve, 100));
host.smartLive = [states.at(-1), hostErrors.slice(errorsBefore)];
await voice.stop();
await voice.setSettings({
  stt: { model: "smoke-stt" }, tts: { model: "smoke-tts" }, end_of_turn: "smart-turn", idle_unload_minutes: 0,
});
host.smartMissing = await code(voice.start());
await until(() => states.at(-1) === "idle");
host.states = states;
await voice.setProviderKey("openai", "sk-smoke");
host.keys = [await voice.hasProviderKey("openai"), stored.get("openai")];
await voice.setProviderKey("openai", null);
host.keys.push(await voice.hasProviderKey("openai"));
host.seam = Object.keys(voice).sort();
host.catalogue = (await voice.models()).length;
console.log(
  JSON.stringify({
    wasm: wasm.pathname,
    loaded: callLoaded,
    ioStarted,
    listenedEarly,
    said,
    state: callState,
    errors,
    webAudioIo: typeof createWebAudioIo,
    host,
  }),
);
