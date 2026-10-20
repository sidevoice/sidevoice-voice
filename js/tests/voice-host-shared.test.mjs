import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const code = await readFile(new URL("../voice-host.js", import.meta.url), "utf8");
const made = [];
const moduleText = code
  .replace('import { VoiceCall as WasmVoiceCall } from "../dist/sidevoice_voice.js";',
    'const WasmVoiceCall = { create(io, setup) { const call = new FakeCall(io, setup); made.push(call); return call; } };')
  .replace('import { createSharedWebAudioIo } from "./shared-web-audio-io.js";',
    'const createSharedWebAudioIo = () => { throw new Error("unexpected browser audio"); };')
  .replace('const fail =', 'const made = globalThis.__made; const FakeCall = globalThis.__FakeCall; const fail =');

// The wasm call as far as the host sees it: it loads what its halves use, slot by slot, opens its lane asking for the
// microphone only with an input half, and says it listens (or is `off` without one).
class FakeCall {
  constructor(io, setup) { this.io = io; this.setup = setup; this.loaded = {}; }
  onEvent(listener) { this.listener = listener; }
  mute() {}
  start() {
    const { input, output } = this.setup;
    const smart = input?.end_of_turn?.kind === "smart";
    Promise.all([
      input ? input.models.vad() : null,
      input ? input.models.transcriber() : null,
      smart ? (input.models.endOfTurn ? input.models.endOfTurn() : Promise.reject(Object.assign(new Error(), { code: "end-of-turn-missing" }))) : null,
      output ? output.models.speaker() : null,
    ]).then(([vad, transcriber, endOfTurn, speaker]) => {
      this.loaded = { vad, transcriber, endOfTurn, speaker };
      const listening = input ? "listening" : "off";
      this.io.start({ ready: () => this.listener({ type: "state", data: { listening } }), captured: () => {} }, { input: !!input });
    }, (error) => this.listener({ type: "error", data: { code: error.code } }));
  }
  stop() { this.io.stop(); this.listener({ type: "state", data: { listening: "idle" } }); }
  // As the call does at its safe points: a half on other models loads them; the same models object loads nothing.
  update(setup) {
    if (setup.input && setup.input.end_of_turn?.kind === "bogus") throw new Error("unknown variant");
    const before = this.setup;
    (this.updates ??= []).push(setup);
    this.setup = setup;
    if (setup.input && setup.input.models !== before.input?.models) { setup.input.models.vad(); setup.input.models.transcriber(); }
    if (setup.output && setup.output.models !== before.output?.models) setup.output.models.speaker();
  }
  free() { this.freed = true; }
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
      const io = { start(sink, options) { this.sink = sink; this.input = options?.input; sink.ready(); }, stop() { this.sink = null; }, dispose() { this.disposed = true; } };
      attached.push({ port, io });
      return { io, input: port };
    },
    async routeInput(port) { routed = port; },
    get routed() { return routed; },
  };
}

/** A source that counts its loads, slot by slot, and gives each detector stream a number. */
function counting(overrides = {}) {
  const loads = { vad: 0, stt: 0, tts: 0, turn: 0 };
  let streams = 0;
  const source = {
    loads,
    vad: async () => { loads.vad++; return { createVad: () => ({ stream: ++streams }) }; },
    transcriber: async (choice) => { loads.stt++; return { choice }; },
    speaker: async (choice) => { loads.tts++; return { choice }; },
    endOfTurn: async (choice) => { loads.turn++; return { choice }; },
    ...overrides,
  };
  return source;
}

const input = { stt: { model: "whisper", language: "es" }, endOfTurn: { kind: "silence" } };
const output = { tts: { model: "kokoro" }, voice: "ef_dora" };
const both = { input, output };

test("two calls share loaded weights but get separate detector streams and audio lanes", async () => {
  const io = audio();
  const source = counting();
  const owner = createVoiceHost(source, { io });
  const a = owner.start(both);
  const b = owner.start(both);
  await Promise.all([a.ready, b.ready]);
  assert.deepEqual(source.loads, { vad: 1, stt: 1, tts: 1, turn: 0 });
  assert.notEqual(made.at(-2).loaded.vad, made.at(-1).loaded.vad);
  assert.equal(made.at(-2).loaded.transcriber, made.at(-1).loaded.transcriber);
  assert.deepEqual(made.at(-1).loaded.transcriber.choice, { model: "whisper" }, "the language chooses no model");
  assert.equal(io.attached.length, 2);
  await owner.routeInput(b);
  assert.equal(io.routed, io.attached[1].port);
  await a.stop();
  assert.equal(io.attached[0].io.disposed, true);
  assert.equal(made.at(-2).freed, true);
  assert.equal(io.attached[1].io.sink !== null, true);
  await b.stop();
  assert.equal(io.attached[1].io.disposed, true);
});

test("a half that is absent is neither loaded nor opened", async () => {
  const io = audio();
  const source = counting();
  const owner = createVoiceHost(source, { io });
  const states = [];
  const listening = owner.start({ output });
  listening.onState((state) => states.push(state.listening));
  await listening.ready;
  assert.deepEqual(source.loads, { vad: 0, stt: 0, tts: 1, turn: 0 }, "a voice try loads no detector nor transcriber");
  assert.equal(io.attached.at(-1).io.input, false, "and asks for no microphone");
  assert.deepEqual(states, ["off"]);
  assert.equal(made.at(-1).setup.input, undefined);
  const transcribing = owner.start({ input: { stt: input.stt, endOfTurn: { kind: "silence", ms: 1500 } } });
  await transcribing.ready;
  assert.deepEqual(source.loads, { vad: 1, stt: 1, tts: 1, turn: 0 }, "a transcription try loads no speaker");
  assert.equal(io.attached.at(-1).io.input, true);
  assert.deepEqual(made.at(-1).setup.input.end_of_turn, { kind: "silence", ms: 1500 });
  assert.equal(made.at(-1).setup.output, undefined);
  await Promise.all([listening.stop(), transcribing.stop()]);
  assert.throws(() => owner.start({}), TypeError);
});

test("silence loads no end-of-turn model; smart loads the one it names", async () => {
  const source = counting();
  const owner = createVoiceHost(source, { io: audio() });
  const silent = owner.start(both);
  await silent.ready;
  assert.equal(source.loads.turn, 0);
  const smart = owner.start({ ...both, input: { ...input, endOfTurn: { kind: "smart", model: { model: "smart-turn-v3" } } } });
  await smart.ready;
  assert.equal(source.loads.turn, 1);
  assert.deepEqual(made.at(-1).loaded.endOfTurn.choice, { model: "smart-turn-v3" });
  assert.deepEqual(made.at(-1).setup.input.end_of_turn, { kind: "smart" });
  await Promise.all([silent.stop(), smart.stop()]);
  const { endOfTurn: _none, ...withoutTurns } = counting();
  const refused = createVoiceHost(withoutTurns, { io: audio() }).start({ input: { ...input, endOfTurn: { kind: "smart" } } });
  await assert.rejects(refused.ready, { code: "end-of-turn-missing" });
  await refused.stop();
});

test("a call after a try reuses the models the try loaded: nothing loads twice", async () => {
  const source = counting();
  const owner = createVoiceHost(source, { io: audio() });
  const transcription = owner.start({ input, idle_unload_minutes: 0 });
  await transcription.ready;
  await transcription.stop();
  // Let go at once with 0 idle minutes: the next one loads again.
  const voice = owner.start({ output, idle_unload_minutes: 10 });
  await voice.ready;
  await voice.stop();
  const call = owner.start({ ...both, idle_unload_minutes: 10 });
  await call.ready;
  assert.deepEqual(source.loads, { vad: 2, stt: 2, tts: 1, turn: 0 }, "the voice try's speaker is the call's");
  const again = owner.start({ ...both, idle_unload_minutes: 10 });
  await again.ready;
  assert.deepEqual(source.loads, { vad: 2, stt: 2, tts: 1, turn: 0 });
  await Promise.all([call.stop(), again.stop()]);
});

test("calls reject routing to an alien or stopped handle", async () => {
  const one = createVoiceHost(counting(), { io: audio() });
  const two = createVoiceHost(counting(), { io: audio() });
  const a = one.start(both);
  await assert.rejects(one.routeInput(two.start(both)), TypeError);
  await a.stop();
  await assert.rejects(one.routeInput(a), TypeError);
});

test("a model that cannot load rejects ready with its code, and a stop before ready rejects it as stopped", async () => {
  const owner = createVoiceHost(counting({ transcriber: async () => { throw Object.assign(new Error("gone"), { code: "model-unknown" }); } }), { io: audio() });
  const failing = owner.start(both);
  const errors = [];
  failing.onError((error) => errors.push(error.code));
  await assert.rejects(failing.ready, { code: "model-unknown" });
  assert.deepEqual(errors, ["model-unknown"]);
  await failing.stop();
  const early = createVoiceHost(counting({ vad: () => new Promise(() => {}) }), { io: audio() }).start(both);
  const stopped = early.stop();
  await assert.rejects(early.ready, { code: "stopped" });
  await stopped;
});

test("a running call's failure releases its model lease at idle", async () => {
  const source = counting();
  const owner = createVoiceHost(source, { io: audio() });
  const a = owner.start({ ...both, idle_unload_minutes: 0 });
  await a.ready;
  const nativeA = made.at(-1);
  nativeA.listener({ type: "error", data: { code: "microphone-lost" } });
  nativeA.listener({ type: "state", data: { listening: "idle" } });
  const b = owner.start({ ...both, idle_unload_minutes: 0 });
  await b.ready;
  assert.equal(source.loads.stt, 2, "a failed call still held in the map does not pin the weights");
  await b.stop();
  await a.stop();
});

test("an active synthesis request keeps its shared choice while another call stops", async () => {
  let finish;
  const source = counting({ speaker: async () => { source.loads.tts++; return { speak: () => new Promise((resolve) => { finish = resolve; }) }; } });
  const owner = createVoiceHost(source, { io: audio() });
  const a = owner.start({ ...both, idle_unload_minutes: 0 });
  const b = owner.start({ ...both, idle_unload_minutes: 0 });
  await Promise.all([a.ready, b.ready]);
  const request = made.at(-1).loaded.speaker.speak("reply");
  await a.stop();
  const c = owner.start({ ...both, idle_unload_minutes: 0 });
  await c.ready;
  assert.equal(source.loads.tts, 1, "B keeps the shared load");
  finish({ samples: new Float32Array(0), sampleRate: 16000 });
  assert.equal((await request).sampleRate, 16000);
  await Promise.all([b.stop(), c.stop()]);
  const d = owner.start({ ...both, idle_unload_minutes: 0 });
  await d.ready;
  assert.equal(source.loads.tts, 2, "the last stop let the choice go");
  await d.stop();
});

test("endInput reaches only its own call, and resolves at once after stop", async () => {
  const owner = createVoiceHost(counting(), { io: audio() });
  const a = owner.start(both);
  const b = owner.start(both);
  await Promise.all([a.ready, b.ready]);
  const nativeA = made.at(-2);
  const nativeB = made.at(-1);
  await a.endInput();
  await a.endInput();
  assert.equal(nativeA.ended, 2);
  assert.equal(nativeB.ended, undefined);
  assert.equal(await a.endInput(), undefined);
  await a.stop();
  await b.stop();
  await a.endInput();
  assert.equal(nativeA.ended, 3, "nothing reaches a stopped call");
});

test("an update keeps the models object of a half whose choices stay, so the call loads nothing again", async () => {
  const source = counting();
  const owner = createVoiceHost(source, { io: audio() });
  const call = owner.start(both);
  await call.ready;
  const native = made.at(-1);
  const { input: first, output: speaking } = native.setup;
  // Another voice: the output half's configuration only; the input half is not in the change, and stays.
  await call.update({ output: { ...output, voice: "other", speed: 1.2 } });
  assert.equal(native.setup.output.models, speaking.models);
  assert.equal(native.setup.input.models, first.models);
  assert.deepEqual([native.setup.output.voice, native.setup.output.speed], ["other", 1.2]);
  // Silence to smart on the same transcriber: the same input models; only the end-of-turn model will load.
  await call.update({ input: { ...input, endOfTurn: { kind: "smart" } } });
  assert.equal(native.setup.input.models, first.models);
  assert.deepEqual(native.setup.input.end_of_turn, { kind: "smart" });
  // The input half goes (the microphone lost, say), then comes back: the transcriber held is not loaded again.
  await call.update({ input: null });
  assert.equal(native.setup.input, undefined);
  await call.update({ input });
  assert.deepEqual(source.loads, { vad: 1, stt: 1, tts: 1, turn: 0 });
  // Another transcriber: a new models object, loaded once.
  await call.update({ input: { ...input, stt: { model: "other" } } });
  assert.notEqual(native.setup.input.models, first.models);
  assert.equal(source.loads.stt, 2);
  await assert.rejects(call.update({ input: null, output: null }), TypeError);
  await assert.rejects(call.update({ input: { ...input, endOfTurn: { kind: "bogus" } } }));
  assert.deepEqual(native.setup.input.end_of_turn, { kind: "silence" }, "a refused update changes nothing");
  await call.stop();
  await assert.rejects(call.update({ output }), { code: "stopped" });
});

const minutes = (count) => count * 60_000;

test("a preload loads what the halves use, and opens no lane", async () => {
  const io = audio();
  const source = counting();
  const owner = createVoiceHost(source, { io });
  const callsBefore = made.length;
  await owner.preload({ output });
  assert.deepEqual(source.loads, { vad: 0, stt: 0, tts: 1, turn: 0 }, "an absent input half loads no detector, transcriber or turn model");
  await owner.preload({ input });
  assert.deepEqual(source.loads, { vad: 1, stt: 1, tts: 1, turn: 0 }, "silence loads no end-of-turn model");
  await owner.preload({ input: { ...input, endOfTurn: { kind: "smart", model: { model: "smart-turn-v3" } } } });
  assert.deepEqual(source.loads, { vad: 1, stt: 1, tts: 1, turn: 1 }, "smart loads the one it names, the rest are held already");
  await owner.preload(both);
  assert.deepEqual(source.loads, { vad: 1, stt: 1, tts: 1, turn: 1 }, "a choice loaded is not loaded again");
  assert.equal(io.attached.length, 0, "no audio lane is attached, so no microphone or speaker is opened");
  assert.equal(made.length, callsBefore, "and no call is made");
  await assert.rejects(owner.preload({}), TypeError);
  await assert.rejects(owner.preload(), TypeError);
  await assert.rejects(owner.preload({ input: null, output: null }), TypeError);
});

test("a call after a preload takes what it loaded, and a preload shares the load of a call", async () => {
  const source = counting();
  const owner = createVoiceHost(source, { io: audio() });
  await owner.preload(both);
  const call = owner.start(both);
  await call.ready;
  assert.deepEqual(source.loads, { vad: 1, stt: 1, tts: 1, turn: 0 }, "the call loads nothing: its models are the preload's");
  assert.equal(made.at(-1).loaded.transcriber.choice.model, "whisper");
  await call.stop();
  // A preload that is still loading when a call asks for the same choice is the same load.
  let finish;
  const slow = counting({ transcriber: (choice) => { slow.loads.stt++; return new Promise((resolve) => { finish = () => resolve({ choice }); }); } });
  const other = createVoiceHost(slow, { io: audio() });
  const warming = other.preload({ input });
  const starting = other.start({ input });
  await new Promise((resolve) => setImmediate(resolve));
  finish();
  await Promise.all([warming, starting.ready]);
  assert.equal(slow.loads.stt, 1);
  await starting.stop();
});

test("a preloaded model is let go idle_unload_minutes after its load, and a call that holds it keeps it", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const source = counting();
  const owner = createVoiceHost(source, { io: audio() });
  await owner.preload({ ...both, idle_unload_minutes: 5 });
  context.mock.timers.tick(minutes(5) - 1);
  const held = owner.start(both);
  await held.ready;
  assert.deepEqual(source.loads, { vad: 1, stt: 1, tts: 1, turn: 0 }, "just before the end of the idle time, the preload is still there");
  context.mock.timers.tick(minutes(60));
  const sharing = owner.start(both);
  await sharing.ready;
  assert.deepEqual(source.loads, { vad: 1, stt: 1, tts: 1, turn: 0 }, "a running call holds what a preload loaded, however long it runs");
  await Promise.all([held.stop(), sharing.stop()]);
  // The idle time restarts when the last holder lets go, with the call's own (10 by default).
  context.mock.timers.tick(minutes(10) - 1);
  await owner.preload(both);
  assert.deepEqual(source.loads, { vad: 1, stt: 1, tts: 1, turn: 0 });
  context.mock.timers.tick(minutes(10));
  const late = owner.start(both);
  await late.ready;
  assert.deepEqual(source.loads, { vad: 2, stt: 2, tts: 2, turn: 0 }, "after the idle time with no holder, the next ask loads again");
  await late.stop();
});

test("a preload with the default idle time lets go after 10 minutes, and 0 holds nothing", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const source = counting();
  const owner = createVoiceHost(source, { io: audio() });
  await owner.preload({ output });
  context.mock.timers.tick(minutes(10) - 1);
  await owner.preload({ output });
  assert.equal(source.loads.tts, 1, "10 minutes less a moment: held; a second preload counts the idle time again");
  context.mock.timers.tick(minutes(10) - 1);
  await owner.preload({ output });
  assert.equal(source.loads.tts, 1);
  context.mock.timers.tick(minutes(10));
  await owner.preload({ output, idle_unload_minutes: 0 });
  assert.equal(source.loads.tts, 2, "10 minutes with no holder: loaded again");
  await owner.preload({ output });
  assert.equal(source.loads.tts, 3, "0 idle minutes let it go as soon as it loaded");
});

test("a preload that cannot load rejects with the code of the slot, keeps what loaded and forgets the refused slot", async () => {
  let refuse = true;
  const source = counting({ transcriber: async (choice) => {
    source.loads.stt++;
    if (refuse) throw Object.assign(new Error("gone"), { code: "model-unknown" });
    return { choice };
  } });
  const owner = createVoiceHost(source, { io: audio() });
  await assert.rejects(owner.preload(both), { code: "model-unknown" });
  assert.deepEqual(source.loads, { vad: 1, stt: 1, tts: 1, turn: 0 });
  refuse = false;
  await owner.preload(both);
  assert.deepEqual(source.loads, { vad: 1, stt: 2, tts: 1, turn: 0 }, "the next ask loads the refused slot again and takes the others held");
  const { endOfTurn: _none, ...withoutTurns } = counting();
  await assert.rejects(createVoiceHost(withoutTurns, { io: audio() }).preload({ input: { ...input, endOfTurn: { kind: "smart" } } }), { code: "end-of-turn-missing" });
});

test("the idle time of a preload counts from the end of its load, however long that takes", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  let finish;
  const source = counting({ transcriber: (choice) => { source.loads.stt++; return new Promise((resolve) => { finish = () => resolve({ choice }); }); } });
  const owner = createVoiceHost(source, { io: audio() });
  const warming = owner.preload({ input, idle_unload_minutes: 10 });
  await new Promise((resolve) => setImmediate(resolve));
  context.mock.timers.tick(minutes(30));
  finish();
  await warming;
  const call = owner.start({ input });
  await call.ready;
  assert.equal(source.loads.stt, 1, "a download that took longer than the idle time is not let go as it ends");
  await call.stop();
});

test("every turn number the host is given reaches the call, and an absent one is left to the call's default", async () => {
  const io = audio();
  const owner = createVoiceHost(counting(), { io });
  const numbers = {
    listening_bar: { quiet: 0.4, playing: 0.9 },
    barge_in_min_speech: 0.3,
    barge_in_window: 4,
    vad_confidence: 0.75,
    speech_start_ms: 250,
    pre_roll_ms: 500,
  };
  const tuned = owner.start({ input: { ...input, ...numbers }, output, audio_grace_ms: 300 });
  await tuned.ready;
  assert.deepEqual(made.at(-1).setup.input.listening_bar, numbers.listening_bar);
  for (const key of ["barge_in_min_speech", "barge_in_window", "vad_confidence", "speech_start_ms", "pre_roll_ms"]) {
    assert.equal(made.at(-1).setup.input[key], numbers[key], key);
  }
  assert.equal(made.at(-1).setup.audio_grace_ms, 300);
  await tuned.update({ input: { ...input, speech_start_ms: 100 }, audio_grace_ms: 50 });
  assert.equal(made.at(-1).updates.at(-1).input.speech_start_ms, 100);
  assert.equal(made.at(-1).updates.at(-1).audio_grace_ms, 50);
  assert.equal("vad_confidence" in made.at(-1).updates.at(-1).input, false);

  const plain = owner.start(both);
  await plain.ready;
  for (const key of [...Object.keys(numbers), "audio_grace_ms"]) {
    assert.equal(key in made.at(-1).setup.input || key in made.at(-1).setup, false, `${key} stays absent`);
  }
  await Promise.all([tuned.stop(), plain.stop()]);
});

test("the output fade is the lane's, 30 ms unless the settings say, and a bad one is refused", async () => {
  const fades = [];
  const io = audio();
  const attach = io.attach;
  io.attach = () => {
    const attached = attach();
    attached.io.setFadeMs = (ms) => fades.push(ms);
    return attached;
  };
  const owner = createVoiceHost(counting(), { io });
  const call = owner.start({ input, output: { ...output, fade_ms: 80 } });
  await call.ready;
  assert.deepEqual(fades, [80]);
  await call.update({ output: { ...output } });
  assert.deepEqual(fades, [80, 30]);
  await assert.rejects(call.update({ output: { ...output, fade_ms: -1 } }), TypeError);
  assert.throws(() => owner.start({ output: { ...output, fade_ms: "slow" } }), TypeError);
  assert.deepEqual(fades, [80, 30], "a refused value changes nothing");
  await call.stop();
});

test("every setting may be left out: the call gets only what the host gave, and a partial listening bar crosses as given", async () => {
  const owner = createVoiceHost(counting(), { io: audio() });
  const bare = owner.start({ input: { stt: {} }, output: {} });
  await bare.ready;
  const { input: bareInput, output: bareOutput, ...bareRest } = made.at(-1).setup;
  assert.deepEqual(Object.keys(bareInput).sort(), ["end_of_turn", "language", "models"]);
  assert.deepEqual(bareInput.end_of_turn, { kind: "silence" });
  assert.deepEqual(Object.keys(bareOutput).sort(), ["language", "models", "voice"]);
  assert.deepEqual(bareRest, { idle_unload_minutes: 0 });
  await bare.stop();
  const partial = owner.start({ input: { stt: {}, endOfTurn: {}, listening_bar: { quiet: 0.3, playing: null }, pre_roll_ms: null } });
  await partial.ready;
  const setup = made.at(-1).setup.input;
  assert.deepEqual(setup.listening_bar, { quiet: 0.3 });
  assert.deepEqual(setup.end_of_turn, { kind: "silence" });
  assert.equal("pre_roll_ms" in setup, false);
  await partial.stop();
});
