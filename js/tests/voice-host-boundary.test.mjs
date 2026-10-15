import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

// Load the real host with a small WASM-call stand-in, so this test runs before the npm artifact is built.
const source = await readFile(new URL("../voice-host.js", import.meta.url), "utf8");
const hostModule = source
  .replace('import { VoiceCall as WasmVoiceCall } from "../dist/sidevoice_voice.js";',
    "const WasmVoiceCall = globalThis.__BoundaryTestCall;")
  .replace('import { createWebAudioIo } from "./web-audio-io.js";',
    "const createWebAudioIo = () => { throw new Error('unexpected browser I/O'); };");

class TestCall {
  static last;
  static create() { return TestCall.last = new TestCall(); }
  onEvent(listener) { this.listener = listener; }
  mute() {}
  captureBoundary() { this.boundaries = (this.boundaries ?? 0) + 1; }
  emit(phase, turn_id, text) {
    this.listener({ type: "turn", data: { phase, turn_id, ...(text ? { text } : {}) } });
  }
}

test("host forwards capture boundary and routes each terminal by its started turn ID", async () => {
  globalThis.__BoundaryTestCall = TestCall;
  const { createVoiceHost } = await import(`data:text/javascript,${encodeURIComponent(hostModule)}`);
  const host = createVoiceHost({ models: () => ({}) }, { io: {} });
  const focus = new Map();
  const delivered = [];
  let selected = "A";
  host.onTurn((turn) => {
    if (turn.phase === "started") focus.set(turn.turn_id, selected);
    if (turn.phase === "finished") delivered.push([focus.get(turn.turn_id), turn.text]);
  });
  await host.setSettings({ stt: {}, tts: {} });
  const call = TestCall.last;

  // A is still in STT when the host changes focus. The terminal can follow B's start.
  call.emit("started", "a-stt");
  host.captureBoundary();
  selected = "B";
  call.emit("started", "b-stt");
  call.emit("finished", "a-stt", "Private instruction for A.");
  call.emit("finished", "b-stt", "Instruction for B.");
  assert.deepEqual(delivered, [
    ["A", "Private instruction for A."],
    ["B", "Instruction for B."],
  ]);

  // A's merge hold releases at the boundary before B starts.
  delivered.length = 0;
  selected = "A";
  call.emit("started", "a-held");
  host.captureBoundary();
  call.emit("finished", "a-held", "Held words for A.");
  selected = "B";
  call.emit("started", "b-held");
  call.emit("finished", "b-held", "Words for B.");
  assert.equal(call.boundaries, 2);
  assert.deepEqual(delivered, [["A", "Held words for A."], ["B", "Words for B."]]);
  delete globalThis.__BoundaryTestCall;
});
