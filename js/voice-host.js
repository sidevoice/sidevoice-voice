// A shared device/model owner with independent calls. Call ids are local and opaque.
import { VoiceCall as WasmVoiceCall } from "../dist/sidevoice_voice.js";
import { createSharedWebAudioIo } from "./shared-web-audio-io.js";

const fail = (code, message) => Object.assign(new Error(message || code), { code });
const canonical = (value) => Array.isArray(value) ? `[${value.map(canonical).join(",")}]`
  : value && typeof value === "object" ? `{${Object.keys(value).sort().filter((key) => value[key] !== undefined).map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`
    : JSON.stringify(value ?? null);
function stages(settings) {
  const { language: _language, ...stt } = settings.stt;
  const { voice: _voice, speed: _speed, ...tts } = settings.tts;
  return canonical([stt, tts, settings.end_of_turn ?? "silence"]);
}
function notPlayed() {
  const outcome = { status: "not-played", reason: "stopped" };
  return Object.freeze({ id: "", cancel() {}, onEvent(listener) { queueMicrotask(() => listener({ type: "done", outcome })); }, outcome: Promise.resolve(outcome) });
}
function devicesOf(listed, kind) {
  return listed.filter((device) => device.kind === kind && device.deviceId && device.deviceId !== "default" && device.deviceId !== "communications")
    .map((device) => ({ id: device.deviceId, label: device.label || "" }));
}

export function createVoiceHost(source, options = {}) {
  const { io: given, ...webAudio } = options;
  let input = webAudio.inputDevice || null;
  let output = webAudio.outputDevice || null;
  let lost = null;
  const deviceListeners = new Set();
  const audio = given ?? createSharedWebAudioIo({ ...webAudio, onDeviceLost: (kind, id) => void deviceLost(kind, id) });
  const calls = new Map();
  const loads = new Map();
  const providers = new Map();
  const users = new Map();
  const idleTimers = new Map();
  let nextId = 0;
  const mediaDevices = () => globalThis.navigator?.mediaDevices;
  async function devices() {
    let listed = [];
    try { listed = (await mediaDevices()?.enumerateDevices?.()) ?? []; } catch (_) { /* unavailable */ }
    const inputs = devicesOf(listed, "audioinput");
    const outputs = devicesOf(listed, "audiooutput");
    if (input && listed.length && !inputs.some((device) => device.id === input)) await deviceLost("input", input, false);
    if (output && listed.length && !outputs.some((device) => device.id === output)) await deviceLost("output", output, false);
    const report = { inputs, outputs, input, output, outputSelectable: audio.outputSelectable === true, ...(lost ? { lost } : {}) };
    lost = null;
    return report;
  }
  async function announce() {
    if (!deviceListeners.size) return;
    const report = await devices();
    for (const listener of deviceListeners) { try { listener(report); } catch (_) { /* listener owns its error */ } }
  }
  async function deviceLost(kind, id, tell = true) {
    if (kind === "input" && input === id) { input = null; await audio.setInputDevice?.(null).catch(() => {}); }
    else if (kind === "output" && output === id) { output = null; await audio.setOutputDevice?.(null).catch(() => {}); }
    else return;
    lost = { ...lost, [kind]: id };
    if (tell) await announce();
  }
  mediaDevices()?.addEventListener?.("devicechange", () => void announce());

  // A rejected load is evicted so the next start may retry. A loaded choice is retained while this owner exists.
  function provider(settings) {
    const key = stages(settings);
    if (!providers.has(key)) {
      providers.set(key, Promise.resolve().then(() => source.models(settings)).catch((error) => { providers.delete(key); throw error; }));
    }
    return providers.get(key);
  }
  function loaded(settings) {
    const key = stages(settings);
    if (!loads.has(key)) {
      const pending = provider(settings).then((modelSource) => modelSource.load());
      loads.set(key, pending.catch((error) => { loads.delete(key); throw error; }));
    }
    return loads.get(key);
  }
  function retain(key, id) {
    clearTimeout(idleTimers.get(key));
    idleTimers.delete(key);
    if (!users.has(key)) users.set(key, new Set());
    users.get(key).add(id);
  }
  function release(key, id, minutes) {
    if (!key) return;
    users.get(key)?.delete(id);
    if (users.get(key)?.size) return;
    clearTimeout(idleTimers.get(key));
    const evict = () => { if (!users.get(key)?.size) { loads.delete(key); providers.delete(key); users.delete(key); } idleTimers.delete(key); };
    if (minutes <= 0) evict();
    else {
      const timer = setTimeout(evict, minutes * 60_000);
      timer.unref?.();
      idleTimers.set(key, timer);
    }
  }
  function createCall() {
    const id = `call-${++nextId}`;
    const { io, input: port } = audio.attach();
    let call = null;
    let chosen = null;
    let active = false;
    let listening = false;
    let stopping = 0;
    let taking = false;
    let muted = false;
    let idleMinutes = 10;
    let disposed = false;
    let waiting = [];
    const answers = [];
    const listeners = { turn: new Set(), state: new Set(), level: new Set(), "playback-level": new Set(), error: new Set(), take: new Set() };
    const settle = (error) => { const pending = waiting; waiting = []; for (const { resolve, reject } of pending) error ? reject(error) : resolve(); };
    function receive(event) {
      const { type: kind, data } = event;
      if (kind === "state") {
        const idle = data.listening === "idle";
        if (stopping > 0) { if (idle) { stopping--; answers.shift()?.(); } }
        else { listening = !idle; if (idle) active = false; else settle(null); }
      } else if (kind === "error" && stopping === 0 && !listening) {
        active = false;
        release(chosen, id, idleMinutes);
        settle(fail(data.code));
      }
      for (const listener of listeners[kind] || []) { try { listener(data); } catch (_) { /* listener owns its error */ } }
    }
    const on = (kind) => (listener) => { if (typeof listener !== "function") throw new TypeError("a listener"); listeners[kind].add(listener); return () => listeners[kind].delete(listener); };
    const hostCall = Object.freeze({
      id,
      async setSettings(settings) {
        if (disposed) throw fail("stopped");
        const key = stages(settings);
        const changed = key !== chosen;
        if (changed) await provider(settings);
        const models = changed ? { load: () => loaded(settings) } : null;
        const config = {
          language: settings.stt.language ?? null, voice: settings.tts.voice ?? null, speed: settings.tts.speed ?? 1,
          end_of_turn: settings.end_of_turn ?? "silence",
          ...(settings.patience ? { patience: settings.patience } : {}),
          ...(settings.idle_unload_minutes != null ? { idle_unload_minutes: settings.idle_unload_minutes } : {}),
          ...(settings.barge_in_min_speech != null ? { barge_in_min_speech: settings.barge_in_min_speech } : {}),
        };
        if (!call) { call = WasmVoiceCall.create(models, io, config); call.onEvent(receive); call.mute(muted); }
        else if (models) call.setModels(models, config);
        else call.setConfig(config);
        if (changed && active) { release(chosen, id, idleMinutes); retain(key, id); }
        chosen = key;
        idleMinutes = settings.idle_unload_minutes ?? 10;
      },
      start(options = {}) {
        if (disposed) return Promise.reject(fail("stopped"));
        if (!call) return Promise.reject(fail("settings-missing", "set the voice settings first"));
        if (listening && stopping === 0) return Promise.resolve();
        const started = new Promise((resolve, reject) => waiting.push({ resolve, reject }));
        if (!active) { active = true; retain(chosen, id); taking = options?.take === true; call.start({ take: taking }); }
        return started;
      },
      async stop() {
        settle(fail("stopped"));
        if (!call) return;
        release(chosen, id, idleMinutes);
        stopping++; active = false; listening = false;
        const kept = taking; taking = false;
        const answered = new Promise((resolve) => answers.push(resolve));
        call.stop();
        if (kept) await answered;
      },
      async dispose() {
        if (disposed) return;
        disposed = true;
        settle(fail("stopped"));
        await hostCall.stop();
        io.dispose?.();
        calls.delete(id);
      },
      say(text, options = {}) { return !disposed && call ? call.say(String(text), options) : notPlayed(); },
      mute(value) { muted = !!value; call?.mute(muted); },
      cancelInput() { call?.cancelInput(); },
      onTurn: on("turn"), onState: on("state"), onLevel: on("level"), onPlaybackLevel: on("playback-level"), onError: on("error"), onTake: on("take"),
    });
    calls.set(id, { hostCall, port });
    return hostCall;
  }
  return Object.freeze({
    createCall,
    routeInput(target) {
      if (target !== null && ![...calls.values()].some(({ hostCall }) => hostCall === target)) return Promise.reject(new TypeError("a call from this host"));
      return audio.routeInput(target === null ? null : calls.get(target.id).port);
    },
    audioDevices: devices,
    async setAudioDevices(choice = {}) {
      if ("input" in choice) {
        const id = choice.input || null;
        if (id && !audio.setInputDevice) throw fail("audio-device-unsupported");
        await audio.setInputDevice?.(id).catch((error) => { throw fail(error?.code || "microphone-unavailable"); });
        input = id;
      }
      if ("output" in choice) {
        const id = choice.output || null;
        if (id && !(audio.outputSelectable === true && audio.setOutputDevice)) throw fail("audio-output-unselectable");
        await audio.setOutputDevice?.(id).catch((error) => { throw fail(error?.code || "audio-output-failed"); });
        output = id;
      }
      return devices();
    },
    onAudioDevices(listener) { deviceListeners.add(listener); return () => deviceListeners.delete(listener); },
  });
}
