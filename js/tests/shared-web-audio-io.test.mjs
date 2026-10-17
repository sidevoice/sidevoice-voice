import assert from "node:assert/strict";
import test from "node:test";
import { createSharedWebAudioIo } from "../shared-web-audio-io.js";

function browser() {
  const all = { opens: 0, closes: 0, tracks: 0, nodes: [], sources: [] };
  const track = () => ({ getSettings: () => ({ echoCancellation: true }), stop: () => { all.tracks++; }, muted: false });
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: { mediaDevices: {
    async getUserMedia() { all.opens++; const t = track(); return { getAudioTracks: () => [t], getTracks: () => [t] }; },
  } } });
  globalThis.AudioWorkletNode = class {
    constructor(_context, name) {
      this.name = name;
      this.port = { close() {}, postMessage: (message) => {
        if (message.type === "route") queueMicrotask(() => this.port.onmessage?.({ data: { type: "routed", generation: message.generation } }));
        if (message.type === "fence") queueMicrotask(() => this.port.onmessage?.({ data: { type: "fence", id: message.id } }));
        if (message.type === "reset") queueMicrotask(() => this.port.onmessage?.({ data: { type: "reset", id: message.id } }));
      } };
      all.nodes.push(this);
    }
    connect(node) { return node; }
    disconnect() {}
  };
  globalThis.AudioContext = class {
    constructor() { this.audioWorklet = { addModule: async () => {} }; this.destination = {}; this.currentTime = 0; this.sampleRate = 16000; }
    createMediaStreamSource() { return { connect: (node) => node, disconnect() {} }; }
    createGain() { return { connect() {}, disconnect() {}, gain: { value: 1, setValueAtTime() {}, linearRampToValueAtTime() {} } }; }
    createBuffer(_channels, length, rate) { return { duration: length / rate, copyToChannel() {} }; }
    createBufferSource() { const source = { connect() {}, start() {}, stop() {} }; all.sources.push(source); return source; }
    async resume() {}
    async close() { all.closes++; }
  };
  return all;
}

test("graph frames survive delayed delivery and only an ended source confirms a chunk", async () => {
  const physical = browser();
  const audio = createSharedWebAudioIo();
  const lane = audio.attach();
  const events = [];
  const heard = sink();
  heard.capturedAt = (_samples, frame) => events.push(["capture", frame]);
  heard.chunkPlayedAt = (id, chunk, frame) => events.push(["completed", id, chunk, frame]);
  lane.io.start(heard);
  for (let i = 0; i < 5 && !heard.events.ready; i++) await new Promise((resolve) => setTimeout(resolve, 0));
  await audio.routeInput(lane.input);
  assert.equal(lane.io.clockFrame(), 128, "publication mark is after the current render quantum");
  lane.io.play("A", 0, new Float32Array(160), 16000);
  assert.deepEqual(events, [], "scheduled output does not certify completed audio");
  const capture = physical.nodes.find((node) => node.name === "sidevoice-capture");
  capture.port.onmessage({data:{type:"captured",generation:2,samples:new Float32Array(160),startFrame:80}});
  assert.deepEqual(events, [["capture",80]]);
  physical.sources[0].onended();
  assert.deepEqual(events, [["capture",80],["completed","A",0,160]]);
  lane.io.stop();
});

test("pending fence suppresses unscheduled output and preserves every scheduled source", async () => {
  const physical=browser();
  const audio=createSharedWebAudioIo();
  const lane=audio.attach(),heard=sink();
  lane.io.start(heard);
  for(let i=0;i<5&&!heard.events.ready;i++)await new Promise(resolve=>setTimeout(resolve,0));
  assert.equal(lane.io.fencePending("not-scheduled"),true);
  lane.io.play("not-scheduled",0,new Float32Array(160),16000);
  assert.equal(physical.sources.length,0);
  lane.io.play("active",0,new Float32Array(160),16000);
  lane.io.play("scheduled",0,new Float32Array(160),16000);
  assert.equal(lane.io.fencePending("active"),false);
  assert.equal(lane.io.fencePending("scheduled"),false,
    "a queued AudioBufferSource may start while the main thread is suspended");
  physical.sources[0].onended();
  assert.equal(lane.io.fencePending("active"),false,"a completed first chunk is still a started utterance");
  lane.io.stop();
});
function sink() {
  const events = { captured: [], rendered: [], ended: 0, ready: 0, chunks: [] };
  return { events, ready() { events.ready++; }, captured(frame) { events.captured.push(frame); },
    rendered(frame) { events.rendered.push(frame); }, playbackEnded() { events.ended++; },
    chunkStarted(id, chunk) { events.chunks.push([id, chunk, "start"]); },
    chunkPlayed(id, chunk) { events.chunks.push([id, chunk, "end"]); },
    microphoneMuted() {}, failed(code) { throw new Error(code); } };
}

test("one physical capture routes the next frame to one lane and zeros to the other", async () => {
  const physical = browser();
  const audio = createSharedWebAudioIo();
  const a = audio.attach(); const b = audio.attach();
  await assert.rejects(audio.routeInput(a.input), TypeError);
  const as = sink(); const bs = sink();
  a.io.start(as); b.io.start(bs);
  for (let i = 0; i < 5 && (!as.events.ready || !bs.events.ready); i++) await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(physical.opens, 1);
  assert.deepEqual([as.events.ready, bs.events.ready], [1, 1]);
  const capture = physical.nodes.find((node) => node.name === "sidevoice-capture");
  await audio.routeInput(a.input);
  capture.port.onmessage({ data: { type: "captured", generation: 2, samples: new Float32Array(160).fill(0.4) } });
  assert.ok(Math.abs(as.events.captured.at(-1)[0] - 0.4) < 0.001);
  assert.equal(bs.events.captured.at(-1)[0], 0);
  await audio.routeInput(b.input);
  capture.port.onmessage({ data: { type: "captured", generation: 3, samples: new Float32Array(160).fill(0.6) } });
  assert.equal(as.events.captured.at(-1)[0], 0);
  assert.ok(Math.abs(bs.events.captured.at(-1)[0] - 0.6) < 0.001);
  a.io.stop();
  assert.equal(physical.closes, 0);
  b.io.stop();
  assert.equal(physical.closes, 1);
  a.io.dispose(); b.io.dispose();
});

test("stopping a routed lane lets another call open, and restart requires a fresh route", async () => {
  const physical = browser();
  const audio = createSharedWebAudioIo();
  const a = audio.attach(); const b = audio.attach();
  const first = sink();
  a.io.start(first);
  for (let i = 0; i < 5 && !first.events.ready; i++) await new Promise((resolve) => setTimeout(resolve, 0));
  await audio.routeInput(a.input);
  a.io.stop();
  const second = sink();
  b.io.start(second);
  for (let i = 0; i < 5 && !second.events.ready; i++) await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(second.events.ready, 1);
  assert.equal(physical.opens, 2);
  const capture = physical.nodes.filter((node) => node.name === "sidevoice-capture").at(-1);
  capture.port.onmessage({ data: { type: "captured", generation: 2, samples: new Float32Array(160).fill(0.8) } });
  assert.equal(second.events.captured.at(-1)[0], 0);
  const restarted = sink();
  a.io.start(restarted);
  for (let i = 0; i < 5 && !restarted.events.ready; i++) await new Promise((resolve) => setTimeout(resolve, 0));
  capture.port.onmessage({ data: { type: "captured", generation: 2, samples: new Float32Array(160).fill(0.8) } });
  assert.equal(restarted.events.captured.at(-1)[0], 0);
  await audio.routeInput(a.input);
  capture.port.onmessage({ data: { type: "captured", generation: 4, samples: new Float32Array(160).fill(0.8) } });
  assert.ok(Math.abs(restarted.events.captured.at(-1)[0] - 0.8) < 0.001);
  a.io.stop(); b.io.stop();
});

test("posted old frames retain their lane until the route fence, including null and successive routes", async () => {
  const physical = browser();
  const audio = createSharedWebAudioIo();
  const a = audio.attach(); const b = audio.attach();
  const as = sink(); const bs = sink();
  a.io.start(as); b.io.start(bs);
  for (let i = 0; i < 5 && (!as.events.ready || !bs.events.ready); i++) await new Promise((resolve) => setTimeout(resolve, 0));
  const capture = physical.nodes.find((node) => node.name === "sidevoice-capture");
  await audio.routeInput(a.input);
  const old = 2;
  const toB = audio.routeInput(b.input);
  capture.port.onmessage({ data: { type: "captured", generation: old, samples: new Float32Array(160).fill(0.4) } });
  assert.ok(Math.abs(as.events.captured.at(-1)[0] - 0.4) < 0.001);
  assert.equal(bs.events.captured.at(-1)[0], 0);
  await toB;
  const toNull = audio.routeInput(null);
  const backToA = audio.routeInput(a.input);
  capture.port.onmessage({ data: { type: "captured", generation: 3, samples: new Float32Array(160).fill(0.6) } });
  assert.ok(Math.abs(bs.events.captured.at(-1)[0] - 0.6) < 0.001);
  await Promise.all([toNull, backToA]);
  capture.port.onmessage({ data: { type: "captured", generation: 4, samples: new Float32Array(160).fill(0.9) } });
  assert.equal(as.events.captured.at(-1)[0], 0);
  assert.equal(bs.events.captured.at(-1)[0], 0);
  capture.port.onmessage({ data: { type: "captured", generation: 5, samples: new Float32Array(160).fill(0.7) } });
  assert.ok(Math.abs(as.events.captured.at(-1)[0] - 0.7) < 0.001);
  a.io.stop(); b.io.stop();
});

test("independent playback meters report only their own lane", async () => {
  const physical = browser();
  const audio = createSharedWebAudioIo();
  const a = audio.attach(); const b = audio.attach();
  const as = sink(); const bs = sink();
  a.io.start(as); b.io.start(bs);
  for (let i = 0; i < 5 && (!as.events.ready || !bs.events.ready); i++) await new Promise((resolve) => setTimeout(resolve, 0));
  const meters = physical.nodes.filter((node) => node.name === "sidevoice-playback");
  assert.equal(meters.length, 2);
  a.io.play("a", 0, new Float32Array(160).fill(0.5), 16000);
  b.io.play("b", 0, new Float32Array(160).fill(0.8), 16000);
  await Promise.resolve();
  meters[0].port.onmessage({ data: { samples: new Float32Array(160).fill(0.5), startFrame: 0, endFrame: 160 } });
  meters[1].port.onmessage({ data: { samples: new Float32Array(160).fill(0.8), startFrame: 0, endFrame: 160 } });
  assert.ok(Math.abs(as.events.rendered.at(-1)[0] - 0.5) < 0.001);
  assert.ok(Math.abs(bs.events.rendered.at(-1)[0] - 0.8) < 0.001);
  a.io.stop(); b.io.stop();
});

test("capture processor stamps its route before the next 10 ms frame", async () => {
  const { readFile } = await import("node:fs/promises");
  const { runInNewContext } = await import("node:vm");
  const source = await readFile(new URL("../capture-worklet.js", import.meta.url), "utf8");
  const posted = [];
  let Processor;
  runInNewContext(source, {
    sampleRate: 16000, Float32Array, Math,
    AudioWorkletProcessor: class { constructor() { this.port = { postMessage: (message) => posted.push(message) }; } },
    registerProcessor: (_name, type) => { Processor = type; },
  });
  const capture = new Processor();
  capture.port.onmessage({ data: { type: "route", generation: 9 } });
  assert.deepEqual(JSON.parse(JSON.stringify(posted)), [{ type: "routed", generation: 9 }]);
  for (let i = 0; i < 3; i++) capture.process([[new Float32Array(128).fill(0.7)]]);
  const first = posted.find((message) => message.type === "captured");
  assert.equal(first.generation, 9);
  assert.equal(first.samples.length, 160);
  capture.port.onmessage({ data: { type: "route", generation: 10 } });
  for (let i = 0; i < 4; i++) capture.process([[new Float32Array(128).fill(0.2)]]);
  assert.ok(posted.some((message) => message.type === "captured" && message.generation === 10));
});

test("capture worklet finishes the old frame before acknowledging a mid-frame route", async () => {
  const { readFile } = await import("node:fs/promises");
  const { runInNewContext } = await import("node:vm");
  const source = await readFile(new URL("../capture-worklet.js", import.meta.url), "utf8");
  const posted = [];
  let Processor;
  runInNewContext(source, {
    sampleRate: 16000, Float32Array, Math,
    AudioWorkletProcessor: class { constructor() { this.port = { postMessage: (message) => posted.push(message) }; } },
    registerProcessor: (_name, type) => { Processor = type; },
  });
  const capture = new Processor();
  capture.port.onmessage({ data: { type: "route", generation: 1 } });
  capture.process([[new Float32Array(128).fill(0.7)]]);
  capture.port.onmessage({ data: { type: "route", generation: 2 } });
  assert.equal(posted.some((message) => message.type === "routed" && message.generation === 2), false);
  capture.process([[new Float32Array(128).fill(0.2)]]);
  const first = posted.find((message) => message.type === "captured");
  assert.equal(first.generation, 1);
  assert.ok(first.samples.slice(0, 120).every((sample) => Math.abs(sample - 0.7) < 0.001));
  assert.ok(posted.indexOf(first) < posted.findIndex((message) => message.type === "routed" && message.generation === 2));
  capture.port.onmessage({ data: { type: "route", generation: 3 } });
  capture.port.onmessage({ data: { type: "route", generation: 4 } });
  for (let i = 0; i < 3; i++) capture.process([[new Float32Array(128).fill(0.3)]]);
  const acks = posted.filter((message) => message.type === "routed").map((message) => message.generation);
  assert.deepEqual(acks, [1, 2, 3, 4]);
});

test("a cut emits terminal zero before successor playback telemetry", async () => {
  const physical = browser();
  const audio = createSharedWebAudioIo();
  const lane = audio.attach();
  const heard = sink();
  lane.io.start(heard);
  for (let i = 0; i < 5 && !heard.events.ready; i++) await new Promise((resolve) => setTimeout(resolve, 0));
  const meter = physical.nodes.find((node) => node.name === "sidevoice-playback");
  lane.io.play("first", 0, new Float32Array(160), 16000);
  await Promise.resolve();
  lane.io.stopPlayback();
  lane.io.play("next", 0, new Float32Array(160), 16000);
  await Promise.resolve();
  meter.port.onmessage({ data: { samples: new Float32Array(160).fill(0.8), startFrame: 480, endFrame: 640 } });
  assert.equal(heard.events.rendered.length, 0, "new telemetry is held behind the old fade");
  await new Promise((resolve) => setTimeout(resolve, 80));
  assert.equal(heard.events.ended, 1);
  assert.ok(Math.abs(heard.events.rendered.at(-1)[0] - 0.8) < 0.001);
  lane.io.stop();
});

test("restarting one lane cannot deliver an old meter report to its new sink", async () => {
  const physical = browser();
  const audio = createSharedWebAudioIo();
  const a = audio.attach(); const b = audio.attach();
  const old = sink(); const other = sink();
  a.io.start(old); b.io.start(other);
  for (let i = 0; i < 5 && !old.events.ready; i++) await new Promise((resolve) => setTimeout(resolve, 0));
  const oldMeter = physical.nodes.find((node) => node.name === "sidevoice-playback");
  a.io.play("old", 0, new Float32Array(160), 16000);
  a.io.stop();
  const next = sink();
  a.io.start(next);
  for (let i = 0; i < 5 && !next.events.ready; i++) await new Promise((resolve) => setTimeout(resolve, 0));
  a.io.play("new", 0, new Float32Array(160), 16000);
  await Promise.resolve();
  oldMeter.port.onmessage({ data: { samples: new Float32Array(160).fill(0.9), startFrame: 0, endFrame: 160 } });
  assert.equal(next.events.rendered.length, 0);
  assert.equal(other.events.ready, 1);
  a.io.stop(); b.io.stop();
});
