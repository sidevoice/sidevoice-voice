// The voice seam over this package's call (`createVoiceHost`): `VoiceHost` (voice-host.d.ts), on a `WebEngine` and
// the browser's microphone and speaker. The Sidevoice desktop app implements the same seam natively.
import { VoiceCall as WasmVoiceCall } from "../dist/sidevoice_voice.js";
import { createWebAudioIo } from "./web-audio-io.js";

/** The voice activity detector every call runs. */
const VAD_MODEL = "silero-vad";
const KEY_PREFIX = "sidevoice.provider-key.";

const fail = (code, message) => Object.assign(new Error(message || code), { code });

/** Provider keys in the page's `localStorage`. */
export function localStorageProviderKeys() {
  return {
    get: (provider) => globalThis.localStorage.getItem(KEY_PREFIX + provider),
    set: (provider, key) =>
      key == null ? globalThis.localStorage.removeItem(KEY_PREFIX + provider) : globalThis.localStorage.setItem(KEY_PREFIX + provider, key),
  };
}

export function createVoiceHost(engine, options = {}) {
  const { io, keys = localStorageProviderKeys(), ...webAudio } = options;
  const listeners = { "user-turn": new Set(), playback: new Set(), state: new Set(), level: new Set(),
    karaoke: new Set(), error: new Set() };
  let call = null;
  let listening = false;
  let waiting = [];

  const settle = (error) => {
    const settled = waiting;
    waiting = [];
    for (const { resolve, reject } of settled) error ? reject(error) : resolve();
  };

  /** One event of the call, `{type, data}`, to its listeners; the lifecycle first. */
  function receive(event) {
    let kind = event.type;
    let data = event.data;
    if (kind === "state") {
      listening = data.listening !== "idle";
      if (listening) settle(null);
    } else if (kind === "error") {
      settle(fail(data.code));
    } else if (kind === "room-message") {
      kind = { "voice-user-turn": "user-turn", "voice-playback": "playback" }[data.type];
      data = data.data;
    }
    for (const listener of (kind && listeners[kind]) || []) {
      try { listener(data); } catch (_) { /* a listener's error is the page's own */ }
    }
  }

  const on = (kind) => (listener) => {
    if (typeof listener !== "function") throw new TypeError("a listener");
    listeners[kind].add(listener);
    return () => listeners[kind].delete(listener);
  };

  /** The model `id`, if the engine's catalogue has it and it can do `task`. */
  async function check(id, task) {
    const model = (await engine.models()).find((candidate) => candidate.id === id);
    if (!model) throw fail("model-unknown", `${id} is not in the engine's catalogue`);
    if (!model.capabilities.includes(task)) throw fail("model-wrong-task", `${id} cannot do ${task}`);
  }

  const withCall = (action) => {
    if (call) action(call);
  };

  return Object.freeze({
    async setSettings(settings) {
      await check(settings.stt.model, "stt");
      await check(settings.tts.model, "tts");
      const config = {
        vad: { model: VAD_MODEL },
        stt: { model: settings.stt.model, language: settings.stt.language ?? null },
        tts: { model: settings.tts.model, voice: settings.tts.voice ?? null, speed: settings.tts.speed ?? 1 },
        ...(settings.patience ? { patience: settings.patience } : {}),
      };
      if (call) {
        call.setConfig(config);
        return;
      }
      call = WasmVoiceCall.create(engine, io ?? createWebAudioIo(webAudio), config);
      call.onEvent(receive);
    },
    start() {
      if (!call) return Promise.reject(fail("settings-missing", "set the voice settings first"));
      if (listening) return Promise.resolve();
      const started = new Promise((resolve, reject) => waiting.push({ resolve, reject }));
      if (waiting.length === 1) call.start();
      return started;
    },
    async stop() {
      settle(fail("stopped"));
      withCall((c) => c.stop());
    },
    speak: (reply) => withCall((c) => c.roomEvent({ type: "voice-reply", data: reply })),
    setOnline: (online) => withCall((c) => c.setOnline(!!online)),
    mute: (muted) => withCall((c) => c.mute(!!muted)),
    cancelInput: () => withCall((c) => c.cancelInput()),
    onUserTurn: on("user-turn"),
    onPlayback: on("playback"),
    onState: on("state"),
    onLevel: on("level"),
    onKaraoke: on("karaoke"),
    onError: on("error"),
    models: () => engine.models(),
    async setProviderKey(provider, key) {
      keys.set(provider, key == null || String(key).trim() === "" ? null : String(key).trim());
    },
    async hasProviderKey(provider) {
      return keys.get(provider) != null;
    },
  });
}
