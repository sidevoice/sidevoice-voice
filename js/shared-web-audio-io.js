// One browser capture and output context with independent call lanes. The microphone is open only while a started
// lane asks for it (`start(sink, {input: true})`, `setInput(true)`): a lane that only speaks asks for no microphone, so a
// blocked one cannot fail it.
const CAPTURE = { echoCancellation: true, noiseSuppression: true, autoGainControl: true, channelCount: 1 };
const FRAME = 160;
const DEFAULT_FADE_MS = 30;
const selectable = () => typeof AudioContext !== "undefined" && typeof AudioContext.prototype.setSinkId === "function";
const failure = (code, message) => Object.assign(new Error(message || code), { code });

export function createSharedWebAudioIo(options = {}) {
  const lanes = new Map();
  let nextPort = 0;
  let selected = null;
  let generation = 0;
  let pendingRoutes = new Map();
  let routes = new Map();
  let context = null;
  let stream = null;
  let microphone = null;
  let capture = null;
  let opening = null;
  let microphoneOpening = null;
  let session = 0;
  let inputDevice = options.inputDevice || null;
  let outputDevice = options.outputDevice || null;
  const frame = () => Math.ceil((context?.currentTime || 0) * (context?.sampleRate || 16000));
  // The browser suspends or interrupts the context on its own (iOS when the page goes to the background or the screen
  // locks, a system interruption) and does not always bring it back: the capture stops, an open turn ends for lack of
  // audio, and nothing is heard or played after. It is resumed whenever it may be: as it changes state while shown,
  // when the page is shown or focused again, at the person's next tap or key, and when the microphone gives audio again.
  const wake = () => {
    if (!context || context.state === "running" || context.state === "closed") return;
    if (globalThis.document?.visibilityState === "hidden") return;
    context.resume().catch(() => {});
  };
  const WAKERS = [["document", "visibilitychange"], ["window", "pageshow"], ["window", "focus"], ["window", "pointerdown"], ["window", "keydown"]];
  const listen = (on) => {
    for (const [where, type] of WAKERS) {
      const target = where === "document" ? globalThis.document : globalThis;
      target?.[on ? "addEventListener" : "removeEventListener"]?.(type, wake, true);
    }
  };

  function releaseDevices() {
    session++;
    for (const pending of pendingRoutes.values()) pending.reject(failure("stopped"));
    pendingRoutes = new Map();
    routes = new Map();
    selected = null;
    closeMicrophone();
    capture?.disconnect(); capture?.port?.close(); capture = null;
    for (const lane of lanes.values()) {
      lane.gain?.disconnect(); lane.meter?.disconnect(); lane.meter?.port?.close();
      lane.gain = lane.meter = null;
      lane.sources.clear(); lane.queueEnd = 0;
      lane.active = null; lane.closing.clear(); lane.deferred = null; lane.rendering = false;
    }
    listen(false);
    if (context) context.onstatechange = null;
    context?.close().catch(() => {}); context = null;
    opening = null;
    microphoneOpening = null;
  }
  function closeMicrophone() {
    for (const track of stream?.getTracks() ?? []) { track.onended = track.onmute = track.onunmute = null; track.stop(); }
    stream = null;
    microphone?.disconnect(); microphone = null;
  }
  const listening = () => [...lanes.values()].filter((lane) => lane.sink && lane.input);
  // The microphone went and none took its place, or it could not open: the lanes that asked for it are told, it is
  // closed, and the speaker goes on.
  function loseInput(code) {
    closeMicrophone();
    for (const lane of listening()) { lane.input = false; lane.sink.inputLost(code); }
  }
  // Opens the microphone for the lanes that ask for it, once; closes it when none does.
  async function microphoneAsAsked() {
    if (!listening().length) { closeMicrophone(); return; }
    if (stream) return;
    if (!microphoneOpening) {
      const opened = session;
      microphoneOpening = media(inputDevice).then((next) => {
        if (opened !== session || !listening().length) { next.getTracks().forEach((track) => track.stop()); return; }
        useStream(next, opened);
      }, (error) => { if (opened === session) loseInput(error?.code || "microphone-unavailable"); })
        .finally(() => { if (opened === session) microphoneOpening = null; });
    }
    await microphoneOpening;
  }
  function failAll(code) {
    const current = [...lanes.values()].filter((lane) => lane.sink);
    for (const lane of current) lane.sink.failed(code);
    for (const lane of current) lane.io.stop();
  }
  async function media(id) {
    if (!globalThis.navigator?.mediaDevices?.getUserMedia) throw failure("microphone-unavailable");
    try {
      const result = await navigator.mediaDevices.getUserMedia({ audio: { ...CAPTURE, ...(id ? { deviceId: { exact: id } } : {}) } });
      if (result.getAudioTracks().some((track) => track.getSettings?.().echoCancellation === false)) {
        result.getTracks().forEach((track) => track.stop());
        throw failure("echo-cancellation-unavailable");
      }
      return result;
    } catch (error) {
      if (error.code) throw error;
      throw failure(error.name === "NotAllowedError" || error.name === "SecurityError" ? "microphone-denied" : "microphone-unavailable", error.message);
    }
  }
  function useStream(next, opened) {
    for (const track of stream?.getTracks() ?? []) { track.onended = track.onmute = track.onunmute = null; track.stop(); }
    stream = next;
    for (const track of next.getAudioTracks()) {
      // The microphone went (unplugged, a route change, taken by the system): the system's default takes its place, as
      // the native capture does; only when none opens do the calls fail.
      track.onended = async () => {
        if (opened !== session) return;
        const old = inputDevice; inputDevice = null;
        try { useStream(await media(null), opened); if (old) options.onDeviceLost?.("input", old); }
        catch (_) { if (opened === session) loseInput("microphone-lost"); }
      };
      track.onmute = () => { if (opened === session) for (const lane of listening()) lane.sink.microphoneMuted(true); };
      track.onunmute = () => { if (opened !== session) return; wake(); for (const lane of listening()) lane.sink.microphoneMuted(false); };
    }
    if (capture) { microphone?.disconnect(); microphone = context.createMediaStreamSource(next); microphone.connect(capture); }
  }
  async function open(opened) {
    context = new AudioContext();
    context.onstatechange = wake;
    listen(true);
    await context.audioWorklet.addModule(new URL("./capture-worklet.js", import.meta.url).href);
    await context.audioWorklet.addModule(new URL("./playback-worklet.js", import.meta.url).href);
    if (opened !== session) return;
    capture = new AudioWorkletNode(context, "sidevoice-capture");
    capture.port.onmessage = ({ data }) => {
      if (opened !== session) return;
      if (data?.type === "routed") {
        const pending = pendingRoutes.get(data.generation);
        if (pending) {
          selected = pending.target; pending.resolve(); pendingRoutes.delete(data.generation);
          for (const past of routes.keys()) if (past < data.generation) routes.delete(past);
        }
        return;
      }
      if (data?.type !== "captured") return;
      const target = routes.get(data.generation) ?? null;
      for (const lane of lanes.values()) if (lane.sink) lane.sink.captured(lane.port === target ? data.samples : new Float32Array(FRAME));
    };
    capture.onprocessorerror = () => failAll("audio-capture-failed");
    // Connected through to the destination so that every browser runs it; it outputs silence. The microphone joins it
    // when a lane asks for one.
    capture.connect(context.destination);
    if (stream) { microphone = context.createMediaStreamSource(stream); microphone.connect(capture); }
    if (outputDevice && selectable()) await context.setSinkId(outputDevice);
    await context.resume();
    if (opened !== session) return;
    // The worklet acknowledges the initial route before any lane reports ready.
    await route(null);
  }
  async function ensureOpen() {
    if (opening) return opening;
    if (context && capture) return;
    if (!opening) {
      const opened = session;
      opening = open(opened).catch((error) => {
        const code = error?.code || "audio-output-failed";
        failAll(code);
        releaseDevices();
        throw error;
      });
    }
    await opening;
  }
  function route(port) {
    if (port !== null && !lanes.get(port)?.sink) return Promise.reject(new TypeError("an active input port"));
    if (!capture) { selected = port; return Promise.resolve(); }
    const serial = ++generation;
    routes.set(serial, port);
    return new Promise((resolve, reject) => {
      pendingRoutes.set(serial, { target: port, resolve, reject });
      capture.port.postMessage({ type: "route", generation: serial });
    });
  }
  function outputFor(lane) {
    if (lane.meter) return;
    lane.meter = new AudioWorkletNode(context, "sidevoice-playback", { outputChannelCount: [1] });
    const meter = lane.meter;
    lane.meter.port.onmessage = ({ data }) => {
      if (!lane.sink || lane.meter !== meter) return;
      if (data?.type === "fence") {
        if (!lane.closing.delete(data.id)) return;
        lane.sink.playbackEnded();
        if (lane.closing.size === 0 && lane.deferred && lane.active?.id === lane.deferred.id) lane.sink.rendered(lane.deferred.samples);
        if (lane.closing.size === 0) lane.deferred = null;
        return;
      }
      if (data?.type === "reset") { if (lane.active?.id === data.id) lane.active.ready = true; return; }
      lane.meter.port.postMessage("ack");
      if (!(data?.samples instanceof Float32Array) || frame() - data.endFrame > (context.sampleRate || 16000) / 4) return;
      const epoch = [...lane.closing.values()].find((past) => data.startFrame >= past.startFrame && data.endFrame <= past.endFrame)
        ?? (lane.active && data.startFrame >= lane.active.startFrame ? lane.active : null);
      if (!epoch || (epoch.endFrame !== undefined && data.endFrame > epoch.endFrame) || (epoch === lane.active && !epoch.ready)) return;
      if (epoch === lane.active && lane.closing.size > 0) lane.deferred = { id: epoch.id, samples: data.samples };
      else lane.sink.rendered(data.samples);
    };
    lane.meter.onprocessorerror = () => { if (lane.meter === meter) lane.sink?.failed("audio-output-failed"); };
    lane.meter.connect(context.destination);
    lane.gain = context.createGain();
    lane.gain.connect(lane.meter);
  }
  function closePlayback(lane, epoch, endTime, waitMs = 0) {
    if (!epoch || lane.closing.has(epoch.id)) return;
    epoch.endFrame = Math.ceil(endTime * (context?.sampleRate || 16000));
    if (lane.active === epoch) lane.active = null;
    lane.rendering = false;
    lane.closing.set(epoch.id, epoch);
    const opened = session;
    const meter = lane.meter;
    setTimeout(() => {
      if (opened === session && meter === lane.meter && lane.closing.has(epoch.id)) meter.port.postMessage({ type: "fence", id: epoch.id });
    }, waitMs);
  }
  function stopPlayback(lane) {
    if (!lane.gain || !context) return;
    lane.playback++;
    const now = context.currentTime;
    lane.gain.gain.setValueAtTime(lane.gain.gain.value, now);
    const fade = lane.fadeMs / 1000;
    lane.gain.gain.linearRampToValueAtTime(0, now + fade);
    for (const source of lane.sources) { source.onended = null; source.stop(now + fade); }
    lane.sources.clear();
    const old = lane.gain;
    closePlayback(lane, lane.active, now + fade, lane.fadeMs * 2);
    setTimeout(() => old.disconnect(), lane.fadeMs * 2);
    lane.gain = context.createGain(); lane.gain.connect(lane.meter);
    lane.queueEnd = 0;
  }
  return {
    get outputSelectable() { return selectable(); },
    attach() {
      const port = Object.freeze({ id: ++nextPort });
      const lane = { port, sink: null, input: false, meter: null, gain: null, sources: new Set(), queueEnd: 0, playback: 0,
        active: null, closing: new Map(), deferred: null, rendering: false, fadeMs: DEFAULT_FADE_MS };
      const io = {
        start(sink, { input = false } = {}) {
          lane.sink = sink;
          lane.input = input === true;
          ensureOpen().then(() => microphoneAsAsked()).then(() => {
            if (!lane.sink || lane.sink !== sink) return;
            outputFor(lane);
            sink.ready();
            if (lane.input && stream?.getAudioTracks().some((track) => track.muted)) sink.microphoneMuted(true);
          }, (error) => { if (lane.sink === sink) sink.failed(error?.code || "audio-output-failed"); });
        },
        setFadeMs(ms) { lane.fadeMs = ms; },
        setInput(input) {
          if (!lane.sink) return;
          lane.input = input === true;
          void microphoneAsAsked();
        },
        play(utterance, chunk, samples, sampleRate) {
          if (!lane.sink || !context || !lane.gain) return;
          if (!lane.rendering) {
            lane.playback++;
            const startFrame = Math.ceil(Math.max(context.currentTime, lane.queueEnd) * context.sampleRate);
            lane.active = { id: lane.playback, startFrame, ready: false };
            lane.meter.port.postMessage({ type: "reset", id: lane.active.id,
              frame: Math.max(startFrame, ...[...lane.closing.values()].map((past) => past.endFrame)) });
            lane.rendering = true;
          }
          const epoch = lane.active;
          const played = lane.playback;
          const buffer = context.createBuffer(1, Math.max(samples.length, 1), sampleRate);
          buffer.copyToChannel(samples, 0);
          const source = context.createBufferSource();
          source.buffer = buffer; source.connect(lane.gain);
          const at = Math.max(context.currentTime, lane.queueEnd);
          lane.queueEnd = at + buffer.duration;
          lane.sources.add(source);
          source.onended = () => {
            lane.sources.delete(source);
            if (played !== lane.playback) return;
            if (lane.sink) lane.sink.chunkPlayed(utterance, chunk);
            if (!lane.sources.size) { lane.playback++; closePlayback(lane, epoch, lane.queueEnd); }
          };
          source.start(at);
          const started = () => {
            if (!lane.sink || !lane.sources.has(source)) return;
            const left = at - context.currentTime;
            if (left > 0) setTimeout(started, left * 1000);
            else lane.sink.chunkStarted(utterance, chunk);
          };
          started();
        },
        stopPlayback() { stopPlayback(lane); },
        stop() {
          stopPlayback(lane);
          lane.sink = null;
          let routeWasHere = selected === port;
          if (routeWasHere) selected = null;
          for (const [serial, target] of routes) if (target === port) routes.set(serial, null);
          for (const [serial, pending] of pendingRoutes) {
            if (pending.target === port) {
              routeWasHere = true;
              pending.reject(failure("stopped")); pendingRoutes.delete(serial);
            }
          }
          const meter = lane.meter;
          lane.gain?.disconnect();
          lane.gain = lane.meter = null;
          lane.active = null; lane.closing.clear(); lane.deferred = null; lane.rendering = false;
          setTimeout(() => { meter?.disconnect(); meter?.port.close(); }, lane.fadeMs * 2);
          lane.input = false;
          if (![...lanes.values()].some((other) => other.sink)) releaseDevices();
          else {
            void microphoneAsAsked();
            if (routeWasHere && capture) void route(null).catch(() => {});
          }
        },
        dispose() {
          io.stop();
          lane.meter?.disconnect(); lane.meter?.port.close();
          lane.gain?.disconnect();
          lanes.delete(port);
          if (selected === port) selected = null;
        },
      };
      lane.io = io;
      lanes.set(port, lane);
      return { io, input: port };
    },
    routeInput: route,
    async setInputDevice(id) {
      const next = id || null;
      if (!stream) { inputDevice = next; return; }
      const opened = session;
      const mediaStream = await media(next);
      if (opened !== session) { mediaStream.getTracks().forEach((track) => track.stop()); return; }
      inputDevice = next; useStream(mediaStream, opened);
    },
    async setOutputDevice(id) {
      const next = id || null;
      if (!selectable()) { if (next) throw failure("audio-output-unselectable"); return; }
      if (context) await context.setSinkId(next ?? "").catch(() => { throw failure("audio-output-failed"); });
      outputDevice = next;
      for (const lane of lanes.values()) lane.sink?.playbackEnded();
    },
  };
}
