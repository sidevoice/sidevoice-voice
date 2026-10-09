// The voice seam over this package's call (`createVoiceHost`): `VoiceHost` (voice-host.d.ts), on the page's models
// and the browser's microphone and speaker. The page's `source` turns the person's settings into models (which model
// fills each slot is the page's) and lists its catalogue; this file runs the call and its lifecycle. The Sidevoice
// desktop app implements the same seam natively.
import { VoiceCall as WasmVoiceCall } from "../dist/sidevoice_voice.js";
import { createWebAudioIo } from "./web-audio-io.js";

const KEY_PREFIX = "sidevoice.provider-key.";

const fail = (code, message) => Object.assign(new Error(message || code), { code });

/** Provider keys in the page's `localStorage`. */
export function localStorageProviderKeys() {
  return {
    get: (provider) => globalThis.localStorage.getItem(KEY_PREFIX + provider),
    set: (provider, key) =>
      key == null
        ? globalThis.localStorage.removeItem(KEY_PREFIX + provider)
        : globalThis.localStorage.setItem(KEY_PREFIX + provider, key),
  };
}

/** What of the settings chooses models: a change to it gives the call other models. */
function stages(settings) {
  const { stt, tts } = settings;
  return JSON.stringify([stt.model, stt.build ?? null, tts.model, tts.build ?? null, settings.end_of_turn ?? "silence"]);
}

export function createVoiceHost(source, options = {}) {
  const { io, keys = localStorageProviderKeys(), ...webAudio } = options;
  const listeners = { "user-turn": new Set(), playback: new Set(), state: new Set(), level: new Set(),
    karaoke: new Set(), error: new Set() };
  let call = null;
  let chosen = null;
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

  const withCall = (action) => {
    if (call) action(call);
  };

  return Object.freeze({
    async setSettings(settings) {
      const changed = chosen === null || stages(settings) !== chosen;
      const models = changed ? await source.models(settings) : null;
      const config = {
        language: settings.stt.language ?? null,
        voice: settings.tts.voice ?? null,
        speed: settings.tts.speed ?? 1,
        end_of_turn: settings.end_of_turn ?? "silence",
        ...(settings.patience ? { patience: settings.patience } : {}),
        ...(settings.idle_unload_minutes != null ? { idle_unload_minutes: settings.idle_unload_minutes } : {}),
      };
      if (!call) {
        call = WasmVoiceCall.create(models, io ?? createWebAudioIo(webAudio), config);
        call.onEvent(receive);
      } else {
        call.setConfig(config);
        if (models) call.setModels(models);
      }
      chosen = stages(settings);
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
    models: () => source.catalogue(),
    async setProviderKey(provider, key) {
      keys.set(provider, key == null || String(key).trim() === "" ? null : String(key).trim());
    },
    async hasProviderKey(provider) {
      return keys.get(provider) != null;
    },
  });
}
