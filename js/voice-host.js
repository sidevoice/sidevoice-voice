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
  // What the page asked for before or after the call exists: every new call starts with it.
  let online = true;
  let muted = false;
  // The lifecycle: a start asked for and not ended (`active`), the call's last state not idle (`listening`), and a
  // the stops sent whose idle state has not come yet (`stopping`): until each comes, a state from before it is stale.
  let active = false;
  let listening = false;
  let stopping = 0;
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
      const idle = data.listening === "idle";
      if (stopping > 0) {
        // Every stop is answered with an idle state, after all the call said before it.
        if (idle) stopping--;
      } else {
        listening = !idle;
        if (idle) active = false;
        else settle(null);
      }
    } else if (kind === "error") {
      // An error before the call listens ends the start that waits for it.
      if (stopping === 0 && !listening) {
        active = false;
        settle(fail(data.code));
      }
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
        call.setOnline(online);
        call.mute(muted);
      } else if (models) {
        // Other models with the configuration they go with, together: a live call restarts once, on both.
        call.setModels(models, config);
      } else {
        call.setConfig(config);
      }
      chosen = stages(settings);
    },
    start() {
      if (!call) return Promise.reject(fail("settings-missing", "set the voice settings first"));
      if (listening && stopping === 0) return Promise.resolve();
      const started = new Promise((resolve, reject) => waiting.push({ resolve, reject }));
      if (!active) {
        active = true;
        call.start();
      }
      return started;
    },
    async stop() {
      settle(fail("stopped"));
      if (!call) return;
      // The start that may follow goes behind this stop, and waits for a state after its idle.
      stopping++;
      active = false;
      listening = false;
      call.stop();
    },
    speak: (reply) => withCall((c) => c.roomEvent({ type: "voice-reply", data: reply })),
    setOnline(value) {
      online = !!value;
      withCall((c) => c.setOnline(online));
    },
    mute(value) {
      muted = !!value;
      withCall((c) => c.mute(muted));
    },
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
