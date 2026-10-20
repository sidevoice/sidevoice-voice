// The voice seam: `createVoiceHost(source, options)`, an owner of the shared microphone and speaker and of the models its
// calls load, slot by slot. `host.start(settings)` starts a call with the halves the settings have, and loads only what
// they use. A model one call loaded is reused by every other that asks for the same choice while any holds it, and
// let go `idle_unload_minutes` after the last one stopped. `host.preload(settings)` loads the same models ahead of a
// call, held the same way. Call ids are local and opaque.
import { VoiceCall as WasmVoiceCall } from "../dist/sidevoice_voice.js";
import { createSharedWebAudioIo } from "./shared-web-audio-io.js";

// The input half's numbers that cross as they are; the call validates them (ranges in src/config.rs) and applies its own
// default to an absent one. The fade is the host's own, so its range is checked here.
const TURN_NUMBERS = ["listening_bar", "barge_in_min_speech", "barge_in_window", "vad_confidence", "speech_start_ms", "pre_roll_ms"];
const FADE_MS = { name: "output.fade_ms", min: 0, max: 1000 };
function requireNumber({ name, min, max }, value) {
  if (typeof value !== "number" || !Number.isFinite(value) || value < min || value > max) {
    throw new TypeError(`${name} must be a number between ${min} and ${max}`);
  }
  return value;
}
// A number the host gave, without the members of an object it left null: the call fills those with its own defaults.
const withoutNulls = (value) => value && typeof value === "object" && !Array.isArray(value)
  ? Object.fromEntries(Object.entries(value).filter(([, member]) => member != null)) : value;
const fail = (code, message) => Object.assign(new Error(message || code), { code });
const canonical = (value) => Array.isArray(value) ? `[${value.map(canonical).join(",")}]`
  : value && typeof value === "object" ? `{${Object.keys(value).sort().filter((key) => value[key] !== undefined).map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`
    : JSON.stringify(value ?? null);
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
  // The models loaded, by slot and choice: `{model, users, timer}`, `model` a promise, `users` the calls holding it.
  const loads = new Map();
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

  function loadSlot(slot, choice) {
    if (slot === "vad") return source.vad();
    if (slot === "stt") return source.transcriber(choice);
    if (slot === "tts") return source.speaker(choice);
    if (typeof source.endOfTurn !== "function") throw fail("end-of-turn-missing");
    return source.endOfTurn(choice);
  }
  // The model of `slot` for `choice`, held by call `id` from now: the one loaded, or loaded now. A rejected load is
  // forgotten, so the next ask loads again.
  const keyOf = (slot, choice) => `${slot} ${canonical(choice)}`;
  function acquire(id, slot, choice) {
    const key = keyOf(slot, choice);
    let entry = loads.get(key);
    if (!entry) {
      entry = { users: new Set(), timer: null, model: null };
      const loading = entry;
      entry.model = Promise.resolve().then(() => loadSlot(slot, choice)).catch((error) => {
        if (loads.get(key) === loading) loads.delete(key);
        throw error;
      });
      loads.set(key, entry);
    }
    clearTimeout(entry.timer);
    entry.timer = null;
    entry.users.add(id);
    return entry.model;
  }
  // Call `id` holds the model filed under `key` no longer: unless another call holds it, it is let go after `minutes`
  // (at once with 0).
  function release(key, entry, id, minutes) {
    if (!entry.users.delete(id) || entry.users.size) return;
    clearTimeout(entry.timer);
    const evict = () => { if (loads.get(key) === entry && !entry.users.size) loads.delete(key); };
    if (minutes <= 0) evict();
    else {
      entry.timer = setTimeout(evict, minutes * 60_000);
      entry.timer.unref?.();
    }
  }
  // Call `id` holds nothing from now.
  function releaseAll(id, minutes) {
    for (const [key, entry] of loads) release(key, entry, id, minutes);
  }
  // The model choices of an input half: its transcriber (its language is no part of it) and end-of-turn model.
  function inputChoices(input) {
    const { language: _language, ...stt } = input.stt ?? {};
    return { stt, turn: input.endOfTurn?.model ?? null };
  }
  // The models `settings` use, each as the slot and choice `acquire` files it under.
  function slotsOf(settings) {
    const slots = [];
    if (settings.input) {
      const { stt, turn } = inputChoices(settings.input);
      slots.push(["vad", null], ["stt", stt]);
      if (settings.input.endOfTurn?.kind === "smart") slots.push(["end-of-turn", turn]);
    }
    if (settings.output) slots.push(["tts", settings.output.tts ?? {}]);
    return slots;
  }
  const keysOf = (settings) => new Set(slotsOf(settings).map(([slot, choice]) => keyOf(slot, choice)));
  // Call `id` holds only what `keys` names from now.
  function releaseUnused(id, keys, minutes) {
    for (const [key, entry] of loads) if (!keys.has(key)) release(key, entry, id, minutes);
  }
  // The input half as the call takes it: its models, each slot asked for when the call needs it, and its configuration.
  // The same choices keep the same models object (`kept`), which the call takes for the same models: it loads nothing
  // again for them.
  function inputOf(id, input, kept) {
    const { language = null } = input.stt ?? {};
    const { stt, turn: chosen } = inputChoices(input);
    const key = canonical([stt, chosen]);
    if (kept.inputKey !== key) {
      kept.inputKey = key;
      kept.input = {
        vad: () => acquire(id, "vad", null).then((detector) => detector.createVad()),
        transcriber: () => acquire(id, "stt", stt),
        endOfTurn: () => acquire(id, "end-of-turn", chosen),
      };
    }
    const { kind = "silence", model: _model, ...turn } = input.endOfTurn ?? {};
    return {
      models: kept.input,
      language,
      end_of_turn: { kind, ...turn },
      ...(input.patience ? { patience: input.patience } : {}),
      ...Object.fromEntries(TURN_NUMBERS.filter((key) => input[key] != null).map((key) => [key, withoutNulls(input[key])])),
    };
  }
  function outputOf(id, output, kept) {
    const { voice = null, speed, language = null } = output;
    const tts = output.tts ?? {};
    const key = canonical(tts);
    if (kept.outputKey !== key) {
      kept.outputKey = key;
      kept.output = { speaker: () => acquire(id, "tts", tts) };
    }
    return { models: kept.output, voice, language, ...(speed != null ? { speed } : {}) };
  }
  function setupOf(id, settings, kept, io) {
    io.setFadeMs?.(settings.output?.fade_ms == null ? 30 : requireNumber(FADE_MS, settings.output.fade_ms));
    if (!settings.input) kept.inputKey = null;
    if (!settings.output) kept.outputKey = null;
    return {
      ...(settings.input ? { input: inputOf(id, settings.input, kept) } : {}),
      ...(settings.output ? { output: outputOf(id, settings.output, kept) } : {}),
      ...(settings.audio_grace_ms != null ? { audio_grace_ms: settings.audio_grace_ms } : {}),
      // The host keeps what a stopped call loaded (`releaseAll`); the call itself drops it as it stops.
      idle_unload_minutes: 0,
    };
  }
  function requireHalf(settings) {
    if (!settings || typeof settings !== "object" || (!settings.input && !settings.output)) {
      throw new TypeError("voice settings with an input or an output half");
    }
  }
  // Loads the models `settings` use, as a start of them would, and holds them as a stopped call does: let go
  // `idle_unload_minutes` after the last holder, counted from the end of the load. A call that starts meanwhile shares
  // the load.
  async function preload(settings) {
    requireHalf(settings);
    const id = `preload-${++nextId}`;
    const settled = await Promise.allSettled(slotsOf(settings).map(([slot, choice]) => acquire(id, slot, choice)));
    releaseAll(id, settings.idle_unload_minutes ?? 10);
    const refused = settled.find(({ status }) => status === "rejected");
    if (refused) throw refused.reason;
  }
  function start(settings, options = {}) {
    requireHalf(settings);
    const id = `call-${++nextId}`;
    let current = settings;
    let minutes = settings.idle_unload_minutes ?? 10;
    const kept = {};
    const { io, input: port } = audio.attach();
    let call;
    try {
      call = WasmVoiceCall.create(io, setupOf(id, settings, kept, io));
    } catch (error) {
      io.dispose?.();
      throw error;
    }
    let started = false;
    let stopped = false;
    let stopping = null;
    let answer = null;
    let settleReady;
    const ready = new Promise((resolve, reject) => { settleReady = (error) => { settleReady = () => {}; error ? reject(error) : resolve(); }; });
    // A page that does not wait for it still gets its failures through `onError`.
    ready.catch(() => {});
    const listeners = { turn: new Set(), state: new Set(), level: new Set(), "playback-level": new Set(), error: new Set(), take: new Set() };
    function receive(event) {
      const { type: kind, data } = event;
      if (kind === "state") {
        const idle = data.listening === "idle";
        if (stopped) { if (idle) answer?.(); }
        else if (!idle) { started = true; settleReady(null); }
        else if (started) releaseAll(id, minutes);
      } else if (kind === "error" && !stopped && !started) {
        releaseAll(id, minutes);
        settleReady(fail(data.code));
      }
      for (const listener of listeners[kind] || []) { try { listener(data); } catch (_) { /* listener owns its error */ } }
    }
    const on = (kind) => (listener) => { if (typeof listener !== "function") throw new TypeError("a listener"); listeners[kind].add(listener); return () => listeners[kind].delete(listener); };
    const hostCall = Object.freeze({
      id,
      ready,
      stop() {
        if (stopping) return stopping;
        stopped = true;
        settleReady(fail("stopped"));
        const answered = new Promise((resolve) => { answer = resolve; });
        call.stop();
        stopping = answered.then(() => {
          call.free?.();
          io.dispose?.();
          releaseAll(id, minutes);
          calls.delete(id);
        });
        return stopping;
      },
      async update(change = {}) {
        if (stopped) throw fail("stopped");
        const next = { ...current };
        for (const half of ["input", "output"]) if (half in change) next[half] = change[half] ?? null;
        if ("idle_unload_minutes" in change) next.idle_unload_minutes = change.idle_unload_minutes;
        if ("audio_grace_ms" in change) next.audio_grace_ms = change.audio_grace_ms;
        if (!next.input && !next.output) throw new TypeError("voice settings with an input or an output half");
        const before = { ...kept };
        try {
          call.update(setupOf(id, next, kept, io));
        } catch (error) {
          Object.assign(kept, before);
          throw error;
        }
        current = next;
        minutes = next.idle_unload_minutes ?? 10;
        releaseUnused(id, keysOf(next), minutes);
      },
      say(text, options = {}) { return stopped ? notPlayed() : call.say(String(text), options); },
      mute(value) { if (!stopped) call.mute(!!value); },
      cancelInput() { if (!stopped) call.cancelInput(); },
      async endInput() { if (!stopped) await call.endInput(); },
      onTurn: on("turn"), onState: on("state"), onLevel: on("level"), onPlaybackLevel: on("playback-level"), onError: on("error"), onTake: on("take"),
    });
    call.onEvent(receive);
    calls.set(id, { hostCall, port });
    call.start({ take: options?.take === true });
    return hostCall;
  }
  return Object.freeze({
    start,
    preload,
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
