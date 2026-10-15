// The voice seam over this package's call (`createVoiceHost`): `VoiceHost` (voice-host.d.ts), on the page's models
// and the browser's microphone and speaker. The page's `source` turns the person's settings into models: which model
// fills each slot, and how a setting names it, is the page's; this file runs the call and its lifecycle. The Sidevoice
// desktop app implements the same seam natively.
import { VoiceCall as WasmVoiceCall } from "../dist/sidevoice_voice.js";
import { createWebAudioIo } from "./web-audio-io.js";

const fail = (code, message) => Object.assign(new Error(message || code), { code });

/** `value` as JSON with its objects' keys in order, so the same choice reads the same however it was written. */
function canonical(value) {
  if (Array.isArray(value)) return "[" + value.map(canonical).join(",") + "]";
  if (value && typeof value === "object") {
    return "{" + Object.keys(value).sort().filter((key) => value[key] !== undefined)
      .map((key) => JSON.stringify(key) + ":" + canonical(value[key])).join(",") + "}";
  }
  return JSON.stringify(value ?? null);
}

/** What of the settings chooses models: each slot's choice as the page wrote it, without what the call itself reads
 *  (the language, the voice and its speed), and the end of turn. A change to it gives the call other models. */
function stages(settings) {
  const { language: _language, ...stt } = settings.stt;
  const { voice: _voice, speed: _speed, ...tts } = settings.tts;
  return canonical([stt, tts, settings.end_of_turn ?? "silence"]);
}

/** The handle of something said with no call yet: never played (`stopped`), its one step told to listeners as they
 *  subscribe. */
function notPlayed() {
  const outcome = { status: "not-played", reason: "stopped" };
  return Object.freeze({
    id: "",
    cancel() {},
    onEvent(listener) {
      queueMicrotask(() => listener({ type: "done", outcome }));
    },
    outcome: Promise.resolve(outcome),
  });
}

/** The audio devices the browser lists of `kind`, as `{id, label}`: the default one is not an entry of its own (it is
 *  the choice `null`). Labels are empty until the page may use the microphone. */
function devicesOf(listed, kind) {
  return listed
    .filter((device) => device.kind === kind && device.deviceId && device.deviceId !== "default" && device.deviceId !== "communications")
    .map((device) => ({ id: device.deviceId, label: device.label || "" }));
}

export function createVoiceHost(source, options = {}) {
  const { io: given, ...webAudio } = options;
  const listeners = {
    turn: new Set(), state: new Set(), level: new Set(), "playback-level": new Set(), "agent-interrupted": new Set(), error: new Set(), devices: new Set(), take: new Set(),
  };
  // The chosen microphone and output (null: the system's default), kept across calls like `muted`.
  let input = webAudio.inputDevice || null;
  let output = webAudio.outputDevice || null;
  // A chosen device that went away, said once with the next list.
  let lost = null;
  const io = given ?? createWebAudioIo({ ...webAudio, onDeviceLost: (kind, id) => void deviceLost(kind, id) });
  let call = null;
  let chosen = null;
  // What the page asked for before or after the call exists: every new call starts with it.
  let muted = false;
  // The lifecycle: a start asked for and not ended (`active`), the call's last state not idle (`listening`), and a
  // the stops sent whose idle state has not come yet (`stopping`): until each comes, a state from before it is stale.
  let active = false;
  let listening = false;
  let stopping = 0;
  let waiting = [];
  // Each stop sent, until the idle state that answers it: told then, in order.
  const answers = [];
  // Whether the start running asked for a take: its stop resolves only once the take was handed over.
  let taking = false;

  const settle = (error) => {
    const settled = waiting;
    waiting = [];
    for (const { resolve, reject } of settled) error ? reject(error) : resolve();
  };

  /** One event of the call, `{type, data}`, to its listeners; the lifecycle first. */
  function receive(event) {
    const kind = event.type;
    const data = event.data;
    if (kind === "state") {
      const idle = data.listening === "idle";
      if (stopping > 0) {
        // Every stop is answered with an idle state, after all the call said before it (its take included).
        if (idle) {
          stopping--;
          answers.shift()?.();
        }
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

  const mediaDevices = () => globalThis.navigator?.mediaDevices;
  /** The devices now, and what is chosen; `lost` once, after a chosen one went away. */
  async function devices() {
    let listed = [];
    try {
      listed = (await mediaDevices()?.enumerateDevices?.()) ?? [];
    } catch (_) { /* a browser that lists nothing lists no device */ }
    const inputs = devicesOf(listed, "audioinput");
    const outputs = devicesOf(listed, "audiooutput");
    // A chosen device that is not listed any more is gone: the default one from now.
    if (input && listed.length && !inputs.some((device) => device.id === input)) await deviceLost("input", input, false);
    if (output && listed.length && !outputs.some((device) => device.id === output)) await deviceLost("output", output, false);
    const report = { inputs, outputs, input, output, outputSelectable: io.outputSelectable === true, ...(lost ? { lost } : {}) };
    lost = null;
    return report;
  }
  /** Tells the device listeners what is listed now. */
  async function announce() {
    if (!listeners.devices.size) return;
    const report = await devices();
    for (const listener of listeners.devices) {
      try { listener(report); } catch (_) { /* a listener's error is the page's own */ }
    }
  }
  /** A chosen device went away: the default one is used from now, and the next list says which was lost. */
  async function deviceLost(kind, id, tell = true) {
    if (kind === "input" && input === id) {
      input = null;
      await io.setInputDevice?.(null).catch(() => {});
    } else if (kind === "output" && output === id) {
      output = null;
      await io.setOutputDevice?.(null).catch(() => {});
    } else return;
    lost = { ...lost, [kind]: id };
    if (tell) await announce();
  }
  // The browser says when its devices change (one plugged in or out): listeners are told what is listed then.
  mediaDevices()?.addEventListener?.("devicechange", () => void announce());

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
        ...(settings.barge_in_min_speech != null ? { barge_in_min_speech: settings.barge_in_min_speech } : {}),
      };
      if (!call) {
        call = WasmVoiceCall.create(models, io, config);
        call.onEvent(receive);
        call.mute(muted);
      } else if (models) {
        // Other models with the configuration they go with, together: a live call restarts once, on both.
        call.setModels(models, config);
      } else {
        call.setConfig(config);
      }
      chosen = stages(settings);
    },
    start(options = {}) {
      if (!call) return Promise.reject(fail("settings-missing", "set the voice settings first"));
      if (listening && stopping === 0) return Promise.resolve();
      const started = new Promise((resolve, reject) => waiting.push({ resolve, reject }));
      if (!active) {
        active = true;
        taking = options?.take === true;
        call.start({ take: taking });
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
      const kept = taking;
      taking = false;
      const answered = new Promise((resolve) => answers.push(resolve));
      call.stop();
      // A take is handed to `onTake` as the listening it covers ends, before the idle state that answers the stop.
      if (kept) await answered;
    },
    say(text, options = {}) {
      return call ? call.say(String(text), options) : notPlayed();
    },
    mute(value) {
      muted = !!value;
      withCall((c) => c.mute(muted));
    },
    cancelInput: () => withCall((c) => c.cancelInput()),
    captureBoundary: () => withCall((c) => c.captureBoundary()),
    onTurn: on("turn"),
    onState: on("state"),
    onLevel: on("level"),
    onPlaybackLevel: on("playback-level"),
    onAgentInterrupted: on("agent-interrupted"),
    onError: on("error"),
    onTake: on("take"),
    audioDevices: () => devices(),
    async setAudioDevices(choice = {}) {
      if ("input" in choice) {
        const id = choice.input || null;
        if (id && !io.setInputDevice) throw fail("audio-device-unsupported");
        await io.setInputDevice?.(id).catch((error) => { throw fail(error?.code || "microphone-unavailable"); });
        input = id;
      }
      if ("output" in choice) {
        const id = choice.output || null;
        if (id && !(io.outputSelectable === true && io.setOutputDevice)) throw fail("audio-output-unselectable");
        await io.setOutputDevice?.(id).catch((error) => { throw fail(error?.code || "audio-output-failed"); });
        output = id;
      }
      return devices();
    },
    onAudioDevices: on("devices"),
  });
}
