// `cargo xtask npm-smoke` (xtask/src/npm.rs) runs this from a directory where @sidevoice/voice was installed from
// its tarball, as a consumer installs it: the package as Node resolves it, its wasm (the file named by the first
// argument, relative to the entry point) read from node_modules, and a call started on plain-object models (the
// interfaces of js/voice-models.d.ts) and a plain-object microphone and speaker, and the default web microphone and
// speaker on a fake browser. It prints what it saw; the checks are xtask's.
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { createVoiceHost, createWebAudioIo, initSync, VoiceCall } from "@sidevoice/voice";

const wasm = new URL(process.argv[2], import.meta.resolve("@sidevoice/voice"));
initSync({ module: readFileSync(wasm) });

// The page's models, as the call loads them: each load records the slots it filled.
const loaded = [];
const detector = () => {
  let end = 0;
  let pending = 0;
  return {
    async accept(samples) {
      pending += samples.length;
      const frames = [];
      while (pending >= 512) {
        pending -= 512;
        end += 512;
        frames.push({ end, speech: false, probability: 0 });
      }
      return frames;
    },
    async reset() { end = 0; pending = 0; },
  };
};
const models = {
  async load() {
    loaded.push("vad", "transcriber", "speaker");
    return {
      vad: detector(),
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
const switched = [];
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
  // Its devices chosen by id, each switch recorded; it can choose its output.
  outputSelectable: true,
  async setInputDevice(id) { switched.push(["input", id]); },
  async setOutputDevice(id) { switched.push(["output", id]); },
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
// The microphone's device going quiet and giving audio again: the call's state says each.
const microphone = [];
for (const muted of [true, false]) {
  speaker.microphoneMuted(muted);
  const want = muted ? "muted" : "live";
  for (let waited = 0; state?.microphone !== want && waited < 5000; waited += 10) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  microphone.push(state?.microphone);
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

// The voice seam on a page's source of the same models: refusals by code, flags set before the settings kept, the
// slots handed to the source as the page wrote them and a change of voice that asks it nothing, a start that resolves
// once listening and at once after, a start straight after a stop that listens, the models loaded again after a stop
// with 0 idle minutes, a live switch to smart-turn with an end-of-turn model, and smart-turn refused without one.
const code = (promise) => promise.then(() => "resolved", (error) => error.code);
const asked = [];
const source = {
  models: async (settings) => {
    asked.push(settings);
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
// The browser's audio devices, as it lists them, and its devicechange.
let deviceList = [
  { kind: "audioinput", deviceId: "default", label: "Default" },
  { kind: "audioinput", deviceId: "mic-a", label: "Mic A" },
  { kind: "audiooutput", deviceId: "out-b", label: "Speakers B" },
];
let deviceChange = null;
Object.defineProperty(globalThis, "navigator", { configurable: true, value: { mediaDevices: {
  enumerateDevices: async () => deviceList,
  addEventListener: (name, listener) => { if (name === "devicechange") deviceChange = listener; },
} } });
const voice = createVoiceHost(source, { io });
const playbackLevels = [];
const unsubscribePlayback = voice.onPlaybackLevel((level) => playbackLevels.push(level));
voice.onPlaybackLevel(() => { throw new Error("isolated listener"); });
const secondPlayback = [];
voice.onPlaybackLevel((level) => secondPlayback.push(level));
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
  stt: { catalog: "smoke", model: "smoke-stt", language: "es" }, tts: { catalog: "smoke", model: "smoke-tts" },
  patience: "fast", idle_unload_minutes: 0,
});
host.handed = asked.at(-1).stt;
host.started = await code(voice.start());
speaker.rendered(new Float32Array(512).fill(0.5));
speaker.captured(new Float32Array(512));
await until(() => playbackLevels.some((level) => level > 0));
speaker.playbackEnded();
await until(() => playbackLevels.at(-1) === 0);
unsubscribePlayback();
speaker.rendered(new Float32Array(512).fill(0.5));
speaker.captured(new Float32Array(512));
await until(() => secondPlayback.length > playbackLevels.length);
host.playbackLevels = [playbackLevels.some((level) => level > 0), playbackLevels.at(-1), secondPlayback.some((level) => level > 0)];
// Another voice and speed are the call's own: the source is not asked again.
const asks = asked.length;
await voice.setSettings({
  stt: { model: "smoke-stt", catalog: "smoke", language: "es" }, tts: { catalog: "smoke", model: "smoke-tts", voice: "other", speed: 1.2 },
  patience: "fast", idle_unload_minutes: 0,
});
host.voiceOnly = asked.length - asks;
const first = seen.find((s) => s.listening !== "idle");
host.flags = [first?.listening];
voice.mute(false);
host.again = await code(voice.start());
// Audio devices with the call listening: the list (the default one is no entry: it is null), a choice the io switches
// to at once, and the chosen microphone gone: the default one again, said once with the list.
const listed = await voice.audioDevices();
host.devices = [listed.inputs.map((device) => device.id), listed.outputs.map((device) => device.label), listed.input, listed.outputSelectable];
const chosen = await voice.setAudioDevices({ input: "mic-a", output: "out-b" });
host.chosen = [chosen.input, chosen.output, switched.slice()];
const told = [];
voice.onAudioDevices((devices) => told.push(devices));
deviceList = deviceList.filter((device) => device.deviceId !== "mic-a");
deviceChange();
await until(() => told.length > 0);
host.deviceGone = [told[0].input, told[0].lost?.input ?? null, switched.at(-1), (await voice.audioDevices()).lost ?? null];
// Said with the call listening: heard, its handle telling how.
const sayingHost = voice.say("The host says this.", { language: "en" });
host.said = [typeof sayingHost.cancel, (await sayingHost.outcome).status];
// A start that asks for a take: what the microphone gives from then on, handed to `onTake` before the stop resolves.
await voice.stop();
const takes = [];
voice.onTake((take) => takes.push(take));
await voice.start({ take: true });
speaker.captured(new Float32Array(1600).fill(0.25));
await new Promise((resolve) => setTimeout(resolve, 50));
await voice.stop();
host.take = takes.map((take) => [take.sampleRate, take.samples.constructor.name, take.samples.length, take.ended_at - take.started_at, take.turns.length]);
await voice.start();
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
host.seam = Object.keys(voice).sort();

// The browser's own microphone and speaker (createWebAudioIo) over a fake getUserMedia and Web Audio: a chosen
// microphone, switched mid-call (the new one open before the old one is let go, the worklet kept), the chosen one
// gone (the default one again, said through onDeviceLost, the call going on), the output chosen with setSinkId, and the
// default one gone (the call ends: microphone-lost).
const webAudio = { asked: [], stopped: [], sinks: [], lost: [], failed: [], muted: [], ready: 0 };
let opened = 0;
// Whether the next microphone's track starts muted.
let startsMuted = false;
const media = (deviceId) => {
  const track = {
    id: `track-${++opened}`, muted: startsMuted, onended: null, onmute: null, onunmute: null,
    stop() { webAudio.stopped.push(track.id); },
  };
  return { getTracks: () => [track], getAudioTracks: () => [track], track, deviceId };
};
let lastStream = null;
Object.defineProperty(globalThis, "navigator", { configurable: true, value: { mediaDevices: {
  getUserMedia: async ({ audio }) => { webAudio.asked.push(audio.deviceId?.exact ?? null); return (lastStream = media(audio.deviceId?.exact ?? null)); },
} } });
// Every capture worklet made, the last one first.
const worklets = [];
globalThis.AudioWorkletNode = class {
  constructor() { worklets.unshift(this); this.port = { close() {} }; this.onprocessorerror = null; }
  connect(node) { return node; }
  disconnect() {}
};
globalThis.AudioContext = class {
  constructor() { this.audioWorklet = { addModule: async () => {} }; this.destination = {}; this.currentTime = 0; }
  createMediaStreamSource(stream) { return { stream, connect(node) { return node; }, disconnect() {} }; }
  createGain() { return { connect() {}, disconnect() {}, gain: { value: 1, setValueAtTime() {}, linearRampToValueAtTime() {} } }; }
  async setSinkId(id) { webAudio.sinks.push(id); }
  async resume() {}
  async close() {}
};
const webIo = createWebAudioIo({ inputDevice: "mic-a", onDeviceLost: (kind, id) => webAudio.lost.push([kind, id]) });
webIo.start({
  ready: () => webAudio.ready++, captured() {}, rendered() {}, playbackEnded() {}, chunkStarted() {}, chunkPlayed() {},
  microphoneMuted: (muted) => webAudio.muted.push(muted), failed: (code) => webAudio.failed.push(code),
});
await until(() => webAudio.ready > 0);
await webIo.setInputDevice("mic-b");
await webIo.setOutputDevice("out-c");
lastStream.track.onended();
await until(() => webAudio.lost.length > 0);
lastStream.track.onended();
await until(() => webAudio.failed.length > 0);
webAudio.outputSelectable = webIo.outputSelectable;
webAudio.worklets = worklets.length;
const devicesSeen = structuredClone(webAudio);

// The default microphone again, whose track starts muted, gives audio, then is muted again, and whose capture worklet's
// processor throws once it runs: what the sink hears, in order.
const heard = [];
startsMuted = true;
createWebAudioIo().start({
  ready: () => heard.push("ready"),
  captured() {},
  rendered() {}, playbackEnded() {},
  chunkStarted() {},
  chunkPlayed() {},
  microphoneMuted: (muted) => heard.push(muted ? "muted" : "unmuted"),
  failed: (code) => heard.push(code),
});
await until(() => heard.includes("ready"));
lastStream.track.onunmute?.();
lastStream.track.onmute?.();
worklets[1].onprocessorerror?.(new Event("processorerror"));
await until(() => heard.includes("audio-capture-failed"));
// Exercise the shipped processor itself with samples, not just events injected at its sink.
const workletSource = readFileSync(new URL("./playback-worklet.js", import.meta.resolve("@sidevoice/voice")), "utf8");
const rendered = [];
let Processor;
runInNewContext(workletSource, {
  sampleRate: 16000, Float32Array, Math,
  AudioWorkletProcessor: class { constructor() { this.port = { postMessage: (frame) => rendered.push(frame) }; } },
  registerProcessor: (_name, implementation) => { Processor = implementation; },
});
const processor = new Processor();
let passes = true;
for (let i = 0; i < 4; i++) {
  const input = new Float32Array(128).fill(0.5);
  const output = new Float32Array(128);
  processor.process([[input]], [[output]]);
  passes &&= output.every((sample) => sample === 0.5);
}
if (!passes || rendered.length < 2 || !rendered[0].samples.every((sample) => Math.abs(sample - 0.5) < 0.01)) {
  throw new Error("playback worklet changed its audio or failed to report rendered frames");
}
const resampled = [];
let ResamplingProcessor;
runInNewContext(workletSource, {
  sampleRate: 48000, Float32Array, Math,
  AudioWorkletProcessor: class { constructor() { this.port = { postMessage: (frame) => resampled.push(frame) }; } },
  registerProcessor: (_name, implementation) => { ResamplingProcessor = implementation; },
});
const resampler = new ResamplingProcessor();
for (let i = 0; i < 48; i++) {
  const input = new Float32Array(128).fill(i < 24 ? 0.5 : 0);
  const output = new Float32Array(128);
  resampler.process([[input]], [[output]]);
  resampler.port.onmessage({ data: "ack" });
  if (!output.every((sample) => sample === (i < 24 ? 0.5 : 0))) throw new Error("playback worklet changed 48 kHz output");
}
if (resampled.length !== 12 || resampled.some(({ samples }) => samples.length !== 160 || !samples.every(Number.isFinite))
  || Math.abs(resampled[4].samples[0] - 0.5) > 0.02 || Math.abs(resampled.at(-1).samples[0]) > 0.02) {
  throw new Error("playback worklet did not resample 48 kHz sound and silence into twelve finite frames");
}
// The page stalls: its four posted reports may be old, but its next acknowledgement must release the newest silence.
const stalled = [];
let StalledProcessor;
runInNewContext(workletSource, {
  sampleRate: 16000, Float32Array, Math,
  AudioWorkletProcessor: class { constructor() { this.port = { postMessage: (frame) => stalled.push(frame) }; } },
  registerProcessor: (_name, implementation) => { StalledProcessor = implementation; },
});
const stalledProcessor = new StalledProcessor();
for (let i = 0; i < 160; i++) {
  const input = new Float32Array(128).fill(i < 80 ? 0.8 : 0);
  stalledProcessor.process([[input]], [[new Float32Array(128)]]);
}
if (stalled.length !== 4) throw new Error("playback worklet did not bound in-flight reports");
stalledProcessor.port.onmessage({ data: "ack" });
if (stalled.length !== 5 || !stalled[4].samples.every((sample) => Math.abs(sample) < 0.001)) {
  throw new Error("playback worklet replayed old sound instead of the newest silence");
}
devicesSeen.workletSamples = [passes, rendered.length, true];
// Connect the shipped processor to the shipped Web Audio I/O. Delay its messages around natural completion, a
// cancellation fade, and an output switch: old sound must never reopen the new playback after its terminal zero.
const reports = [];
const playbackSources = [];
let LiveProcessor;
const workletWorld = {
  sampleRate: 16000, currentFrame: 0, Float32Array, Math,
  AudioWorkletProcessor: class { constructor() { this.port = { postMessage: (data) => reports.push(data) }; } },
  registerProcessor: (_name, implementation) => { LiveProcessor = implementation; },
};
runInNewContext(workletSource, workletWorld);
let liveProcessor;
let liveMeter;
let liveContext;
let failPlaybackModule = false;
globalThis.AudioWorkletNode = class {
  constructor(_context, name) {
    if (name === "sidevoice-playback") {
      liveProcessor = new LiveProcessor();
      liveMeter = this;
      this.port = { postMessage: (data) => liveProcessor.port.onmessage({ data }), close() {}, onmessage: null };
    } else this.port = { close() {}, onmessage: null };
  }
  connect(node) { return node; }
  disconnect() {}
};
globalThis.AudioContext = class {
  constructor() { liveContext = this; this.sampleRate = 16000; this.currentTime = 0; this.destination = {}; this.audioWorklet = { addModule: async (url) => { if (failPlaybackModule && url.includes("playback-worklet")) throw new Error("missing playback worklet"); } }; }
  createMediaStreamSource() { return { connect(node) { return node; }, disconnect() {} }; }
  createGain() { return { connect() {}, disconnect() {}, gain: { value: 1, setValueAtTime() {}, linearRampToValueAtTime() {} } }; }
  createBuffer(_channels, length, rate) { return { duration: length / rate, copyToChannel() {} }; }
  createBufferSource() { const source = { connect() {}, start() {}, stop() {}, onended: null }; playbackSources.push(source); return source; }
  async setSinkId() {}
  async resume() {}
  async close() {}
};
const playedLevels = [];
const playbackIo = createWebAudioIo();
playbackIo.start({ ready() {}, captured() {}, rendered: (samples) => playedLevels.push(samples[0]),
  playbackEnded: () => playedLevels.push(0), chunkStarted() {}, chunkPlayed() {}, microphoneMuted() {}, failed: (code) => { throw new Error(code); } });
await until(() => liveMeter);
const processOutput = (sample, blocks) => {
  for (let i = 0; i < blocks; i++) {
    const input = new Float32Array(128).fill(sample);
    const output = new Float32Array(128);
    liveProcessor.process([[input]], [[output]]);
    if (!output.every((value) => Math.abs(value - sample) < 1e-6)) throw new Error("playback processor changed output");
    workletWorld.currentFrame += 128;
    liveContext.currentTime = workletWorld.currentFrame / 16000;
  }
};
const deliver = () => { while (reports.length) liveMeter.port.onmessage({ data: reports.shift() }); };
playbackIo.play("old", 0, new Float32Array(160).fill(0.8), 16000);
processOutput(0.8, 2);
const oldReport = reports.find((data) => data.samples);
playbackSources.at(-1).onended();
await new Promise((resolve) => setTimeout(resolve, 5));
playbackIo.play("new", 0, new Float32Array(160), 16000);
processOutput(0, 3);
deliver();
const firstZero = playedLevels.indexOf(0);
if (firstZero < 0 || !playedLevels.slice(0, firstZero).some((value) => value > 0.5)) {
  throw new Error("natural playback end did not follow rendered sound");
}
liveMeter.port.onmessage({ data: oldReport });
if (playedLevels.slice(firstZero).some((value) => value > 0)) throw new Error("late old frame reopened the new silent playback");
// The old gain stays connected through the fade; reports rendered during it are valid until its fence.
playbackIo.stopPlayback();
playbackIo.play("immediate silent replacement", 0, new Float32Array(160), 16000);
processOutput(0.2, 4);
await new Promise((resolve) => setTimeout(resolve, 70));
deliver();
const fadedZero = playedLevels.lastIndexOf(0);
if (fadedZero < 0 || !playedLevels.slice(firstZero, fadedZero).some((value) => value > 0)) {
  throw new Error("cancellation did not meter its audible fade before terminal zero");
}
processOutput(0, 3);
deliver();
liveMeter.port.onmessage({ data: oldReport });
if (playedLevels.slice(fadedZero).some((value) => value > 0)) throw new Error("late fade frame reopened playback");
// A live output switch is another telemetry boundary; an old frame cannot follow its reset.
processOutput(0.8, 2);
const beforeSwitch = reports.findLast((data) => data.samples);
if (!beforeSwitch) throw new Error("no worklet report to test output replacement");
await playbackIo.setOutputDevice("another-output");
const switchZero = playedLevels.lastIndexOf(0);
liveMeter.port.onmessage({ data: beforeSwitch });
if (playedLevels.slice(switchZero).some((value) => value > 0)) throw new Error("output switch accepted old sound");
playbackIo.stop();
failPlaybackModule = true;
let outputFailure;
createWebAudioIo().start({ ready() {}, captured() {}, rendered() {}, playbackEnded() {}, chunkStarted() {}, chunkPlayed() {},
  microphoneMuted() {}, failed: (code) => { outputFailure = code; } });
await until(() => outputFailure);
if (outputFailure !== "audio-output-failed") throw new Error("playback worklet load failure was mislabeled");
console.log(
  JSON.stringify({
    wasm: wasm.pathname,
    loaded: callLoaded,
    ioStarted,
    listenedEarly,
    said,
    state: callState,
    errors,
    microphone,
    webAudioIo: typeof createWebAudioIo,
    webAudio: { ...devicesSeen, heard },
    host,
  }),
);
