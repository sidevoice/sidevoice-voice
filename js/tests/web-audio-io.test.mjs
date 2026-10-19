import assert from "node:assert/strict";
import test from "node:test";

import { createWebAudioIo } from "../web-audio-io.js";

async function openWithBrowserSetting(echoCancellation) {
  let requested;
  let stopped = false;
  const track = {
    getSettings: () => ({ echoCancellation }),
    stop: () => { stopped = true; },
  };
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: { mediaDevices: {
      getUserMedia: async (constraints) => {
        requested = constraints;
        return { getAudioTracks: () => [track], getTracks: () => [track] };
      },
    } },
  });
  globalThis.AudioContext = class {
    audioWorklet = { addModule: async () => { throw new Error("stop after microphone setup"); } };
    close() { return Promise.resolve(); }
  };
  const io = createWebAudioIo();
  const code = await new Promise((resolve) => io.start({ failed: resolve, inputLost: resolve, microphoneMuted() {} }, { input: true }));
  return { code, requested, stopped };
}

test("browser capture requires AEC before any windows can reach voice", async () => {
  const refused = await openWithBrowserSetting(false);
  assert.equal(refused.requested.audio.echoCancellation, true);
  assert.equal(refused.code, "echo-cancellation-unavailable");
  assert.equal(refused.stopped, true);

  const accepted = await openWithBrowserSetting(true);
  assert.equal(accepted.requested.audio.echoCancellation, true);
  assert.equal(accepted.code, "audio-capture-failed");
});
