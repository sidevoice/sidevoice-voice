// `cargo xtask npm-smoke` (xtask/src/npm.rs) runs this from a directory where @sidevoice/voice was installed from
// its tarball, as a consumer installs it: the package as Node resolves it, its wasm (the file named by the first
// argument, relative to the entry point) read from node_modules, and a call started on plain-object models (the
// interfaces of js/voice-models.d.ts) and a plain-object microphone and speaker. It prints what it saw; the checks
// are xtask's.
import { readFileSync } from "node:fs";
import { createWebAudioIo, initSync, VoiceCall } from "@sidevoice/voice";

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
const io = {
  start(sink) {
    ioStarted = true;
    setTimeout(() => {
      ready = true;
      sink.ready();
      sink.captured(new Float32Array(160));
    }, 50);
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
call.stop();
console.log(
  JSON.stringify({
    wasm: wasm.pathname,
    loaded,
    ioStarted,
    listenedEarly,
    state,
    errors,
    webAudioIo: typeof createWebAudioIo,
  }),
);
