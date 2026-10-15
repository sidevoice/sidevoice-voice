import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const code = await readFile(new URL("../voice-host.js", import.meta.url), "utf8");
const made = [];
const moduleText = code
  .replace('import { VoiceCall as WasmVoiceCall } from "../dist/sidevoice_voice.js";',
    'const WasmVoiceCall = { create(models, io) { const call = new FakeCall(models, io); made.push(call); return call; } };')
  .replace('import { createSharedWebAudioIo } from "./shared-web-audio-io.js";',
    'const createSharedWebAudioIo = () => { throw new Error("unexpected browser audio"); };')
  .replace('const fail =', 'const made = globalThis.__made; const FakeCall = globalThis.__FakeCall; const fail =');
class FakeCall {
  constructor(models, io) { this.models = models; this.io = io; this.events = []; this.says = []; }
  onEvent(listener) { this.listener = listener; }
  mute() {}
  start() {
    this.models.load().then(async (loaded) => {
      this.loaded = { ...loaded, vad: await loaded.createVad() };
      this.io.start({ ready: () => this.listener({ type: "state", data: { listening: "listening" } }), captured: () => {} });
    });
  }
  stop() { this.io.stop(); this.listener({ type: "state", data: { listening: "idle" } }); }
  setConfig() {}
  setModels(models) { this.models = models; }
  cancelInput() {}
  endInput() { this.ended = (this.ended ?? 0) + 1; return Promise.resolve(); }
  say() { return { id: "say", outcome: Promise.resolve({ status: "heard" }), cancel() {}, onEvent() {} }; }
}
globalThis.__made = made;
globalThis.__FakeCall = FakeCall;
const { createVoiceHost } = await import(`data:text/javascript,${encodeURIComponent(moduleText)}`);

function audio() {
  const attached = [];
  let routed = null;
  return {
    attached,
    attach() {
      const port = {};
      const io = { start(sink) { this.sink = sink; sink.ready(); }, stop() { this.sink = null; }, dispose() { this.disposed = true; } };
      attached.push({ port, io });
      return { io, input: port };
    },
    async routeInput(port) { routed = port; },
    get routed() { return routed; },
  };
}
const settings = { stt: { model: "same" }, tts: { model: "same" } };
test("two calls share loaded weights but get separate detector streams and audio lanes", async () => {
  const io = audio();
  let loads = 0;
  let detectors = 0;
  const owner = createVoiceHost({ models: async () => ({ load: async () => {
    loads++;
    return { createVad: () => ({ stream: ++detectors }), transcriber: {}, speaker: {} };
  } }) }, { io });
  const a = owner.createCall();
  const b = owner.createCall();
  await Promise.all([a.setSettings(settings), b.setSettings(settings)]);
  await Promise.all([a.start(), b.start()]);
  assert.equal(loads, 1);
  assert.notEqual(made.at(-2).loaded.vad, made.at(-1).loaded.vad);
  assert.equal(made.at(-2).loaded.transcriber, made.at(-1).loaded.transcriber);
  assert.equal(io.attached.length, 2);
  await owner.routeInput(b);
  assert.equal(io.routed, io.attached[1].port);
  await a.stop();
  assert.equal(loads, 1);
  assert.equal(io.attached[1].io.sink !== null, true);
  await a.dispose();
  assert.equal(io.attached[0].io.disposed, true);
  await b.dispose();
  assert.equal(io.attached[1].io.disposed, true);
});

test("calls reject routing to an alien or disposed handle", async () => {
  const one = createVoiceHost({ models: () => ({}) }, { io: audio() });
  const two = createVoiceHost({ models: () => ({}) }, { io: audio() });
  const a = one.createCall();
  await assert.rejects(one.routeInput(two.createCall()), TypeError);
  await a.dispose();
  await assert.rejects(one.routeInput(a), TypeError);
});

test("a running call's failure releases its model lease at idle", async () => {
  let loads = 0;
  const owner = createVoiceHost({ models: () => ({ load: async () => {
    loads++;
    return { createVad: () => ({}), transcriber: {}, speaker: {} };
  } }) }, { io: audio() });
  const a = owner.createCall();
  await a.setSettings({ ...settings, idle_unload_minutes: 0 });
  await a.start();
  const nativeA = made.at(-1);
  nativeA.listener({ type: "error", data: { code: "microphone-lost" } });
  nativeA.listener({ type: "state", data: { listening: "idle" } });
  const b = owner.createCall();
  await b.setSettings({ ...settings, idle_unload_minutes: 0 });
  await b.start();
  assert.equal(loads, 2, "a failed call still held in the map does not pin the weights");
  await b.stop();
  await a.dispose();
  await b.dispose();
});

test("a second live call keeps shared weights through the first call's failure", async () => {
  let loads = 0;
  const owner = createVoiceHost({ models: () => ({ load: async () => {
    loads++;
    return { createVad: () => ({}), transcriber: {}, speaker: {} };
  } }) }, { io: audio() });
  const a = owner.createCall();
  const b = owner.createCall();
  await Promise.all([a.setSettings({ ...settings, idle_unload_minutes: 0 }), b.setSettings({ ...settings, idle_unload_minutes: 0 })]);
  await Promise.all([a.start(), b.start()]);
  const nativeA = made.at(-2);
  nativeA.listener({ type: "error", data: { code: "audio-output-failed" } });
  nativeA.listener({ type: "state", data: { listening: "idle" } });
  const c = owner.createCall();
  await c.setSettings({ ...settings, idle_unload_minutes: 0 });
  await c.start();
  assert.equal(loads, 1);
  await a.dispose();
  await b.dispose();
  await c.dispose();
});

test("two idle calls evict one shared load and the next use reloads it", async () => {
  let loads = 0;
  const owner = createVoiceHost({ models: () => ({ load: async () => {
    loads++;
    return { createVad: () => ({}), transcriber: {}, speaker: {} };
  } }) }, { io: audio() });
  const a = owner.createCall(); const b = owner.createCall();
  await Promise.all([a.setSettings({ ...settings, idle_unload_minutes: 0 }), b.setSettings({ ...settings, idle_unload_minutes: 0 })]);
  await Promise.all([a.start(), b.start()]);
  assert.equal(loads, 1);
  await a.stop();
  const c = owner.createCall();
  await c.setSettings({ ...settings, idle_unload_minutes: 0 });
  await c.start();
  assert.equal(loads, 1, "B retains the shared choice");
  await Promise.all([b.stop(), c.stop()]);
  const d = owner.createCall();
  await d.setSettings({ ...settings, idle_unload_minutes: 0 });
  await d.start();
  assert.equal(loads, 2, "the final idle lease evicted the choice");
  await Promise.all([a.dispose(), b.dispose(), c.dispose(), d.dispose()]);
});

test("an active synthesis request keeps its shared choice while another call stops", async () => {
  let loads = 0;
  let finish;
  const owner = createVoiceHost({ models: () => ({ load: async () => {
    loads++;
    return { createVad: () => ({}), transcriber: {}, speaker: {
      speak: () => new Promise((resolve) => { finish = resolve; }),
    } };
  } }) }, { io: audio() });
  const a = owner.createCall(); const b = owner.createCall();
  await Promise.all([a.setSettings({ ...settings, idle_unload_minutes: 0 }), b.setSettings({ ...settings, idle_unload_minutes: 0 })]);
  await Promise.all([a.start(), b.start()]);
  const request = made.at(-1).loaded.speaker.speak("reply");
  await a.stop();
  const c = owner.createCall();
  await c.setSettings({ ...settings, idle_unload_minutes: 0 });
  await c.start();
  assert.equal(loads, 1, "B's active request keeps the shared load");
  finish({ samples: new Float32Array(0), sampleRate: 16000 });
  assert.equal((await request).sampleRate, 16000);
  await Promise.all([b.stop(), c.stop()]);
  await Promise.all([a.dispose(), b.dispose(), c.dispose()]);
});

test("endInput resolves at once before settings and reaches only its own call afterwards", async () => {
  const owner = createVoiceHost({ models: async () => ({ load: async () => ({ createVad: () => ({}), transcriber: {}, speaker: {} }) }) }, { io: audio() });
  const a = owner.createCall();
  const b = owner.createCall();
  await a.endInput();
  await Promise.all([a.setSettings(settings), b.setSettings(settings)]);
  await Promise.all([a.start(), b.start()]);
  const nativeA = made.at(-2);
  const nativeB = made.at(-1);
  await a.endInput();
  await a.endInput();
  assert.equal(nativeA.ended, 2);
  assert.equal(nativeB.ended, undefined);
  assert.equal(await a.endInput(), undefined);
  await a.dispose();
  await b.dispose();
  await a.endInput();
});
