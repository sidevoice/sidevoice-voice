// One browser capture and output context with independent call lanes.
const CAPTURE = { echoCancellation: true, noiseSuppression: true, autoGainControl: true, channelCount: 1 };
const FRAME = 160;
const FADE_SECONDS = 0.03;
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
  let session = 0;
  let inputDevice = options.inputDevice || null;
  let outputDevice = options.outputDevice || null;
  const frame = () => Math.ceil((context?.currentTime || 0) * (context?.sampleRate || 16000));

  function releaseDevices() {
    session++;
    for (const pending of pendingRoutes.values()) pending.reject(failure("stopped"));
    pendingRoutes = new Map();
    routes = new Map();
    selected = null;
    for (const track of stream?.getTracks() ?? []) { track.onended = track.onmute = track.onunmute = null; track.stop(); }
    stream = null;
    microphone?.disconnect(); microphone = null;
    capture?.disconnect(); capture?.port?.close(); capture = null;
    for (const lane of lanes.values()) {
      lane.gain?.disconnect(); lane.meter?.disconnect(); lane.meter?.port?.close();
      lane.gain = lane.meter = null;
      lane.sources.clear(); lane.queueEnd = 0;
      lane.active = null; lane.closing.clear(); lane.deferred = null; lane.rendering = false;
    }
    context?.close().catch(() => {}); context = null;
    opening = null;
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
      track.onended = async () => {
        if (opened !== session) return;
        if (!inputDevice) return failAll("microphone-lost");
        const old = inputDevice; inputDevice = null;
        try { useStream(await media(null), opened); options.onDeviceLost?.("input", old); }
        catch (_) { failAll("microphone-lost"); }
      };
      track.onmute = () => { if (opened === session) for (const lane of lanes.values()) lane.sink?.microphoneMuted(true); };
      track.onunmute = () => { if (opened === session) for (const lane of lanes.values()) lane.sink?.microphoneMuted(false); };
    }
    if (capture) { microphone?.disconnect(); microphone = context.createMediaStreamSource(next); microphone.connect(capture); }
  }
  async function open(opened) {
    const mediaStream = await media(inputDevice);
    if (opened !== session) { mediaStream.getTracks().forEach((track) => track.stop()); return; }
    useStream(mediaStream, opened);
    context = new AudioContext();
    await context.audioWorklet.addModule(new URL("./capture-worklet.js", import.meta.url).href);
    await context.audioWorklet.addModule(new URL("./playback-worklet.js", import.meta.url).href);
    if (opened !== session) return;
    microphone = context.createMediaStreamSource(stream);
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
      for (const lane of lanes.values()) if (lane.sink) {
        const samples=lane.port===target?data.samples:new Float32Array(FRAME);
        if(Number.isSafeInteger(data.startFrame)&&lane.sink.capturedAt)lane.sink.capturedAt(samples,data.startFrame);
        else lane.sink.captured(samples);
      }
    };
    capture.onprocessorerror = () => failAll("audio-capture-failed");
    microphone.connect(capture).connect(context.destination);
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
    lane.gain.gain.linearRampToValueAtTime(0, now + FADE_SECONDS);
    for (const source of lane.sources) { source.onended = null; source.stop(now + FADE_SECONDS); }
    lane.sources.clear();
    lane.sourceTimes.clear();
    const old = lane.gain;
    closePlayback(lane, lane.active, now + FADE_SECONDS, FADE_SECONDS * 2000);
    setTimeout(() => old.disconnect(), FADE_SECONDS * 2000);
    lane.gain = context.createGain(); lane.gain.connect(lane.meter);
    lane.queueEnd = 0;
  }
  return {
    get outputSelectable() { return selectable(); },
    attach() {
      const port = Object.freeze({ id: ++nextPort });
      const lane = { port, sink: null, meter: null, gain: null, sources: new Set(), sourceTimes: new Map(),
        startedIds: new Set(), fencedIds: new Set(), fenceOverflow: false, queueEnd: 0, playback: 0,
        active: null, closing: new Map(), deferred: null, rendering: false };
      const io = {
        // currentTime names the render quantum's start. Use the end as a
        // conservative publication watermark for samples in this quantum.
        clockFrame() {return context ? Math.floor(context.currentTime*context.sampleRate)+128 : null;},
        // A room can retract only an utterance with no source already
        // scheduled. A scheduled source may cross its start during this JS
        // task, so it is conservatively treated as possibly active.
        fencePending(utterance) {
          if(!context || lane.fenceOverflow || lane.startedIds.has(utterance))return false;
          if([...lane.sourceTimes.values()].some(id=>id===utterance))return false;
          if(lane.fencedIds.size>=2048){lane.fenceOverflow=true;return false}
          lane.fencedIds.add(utterance);
          return true;
        },
        start(sink) {
          lane.sink = sink;
          ensureOpen().then(() => {
            if (!lane.sink || lane.sink !== sink) return;
            outputFor(lane);
            sink.ready();
            if (stream?.getAudioTracks().some((track) => track.muted)) sink.microphoneMuted(true);
          }, (error) => { if (lane.sink === sink) sink.failed(error?.code || "audio-output-failed"); });
        },
        play(utterance, chunk, samples, sampleRate) {
          if(lane.fencedIds.has(utterance))return;
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
          const endFrame=Math.ceil(lane.queueEnd*context.sampleRate);
          lane.sources.add(source);
          lane.sourceTimes.set(source,utterance);
          source.onended = () => {
            lane.sources.delete(source);
            lane.sourceTimes.delete(source);
            if(!lane.fenceOverflow){lane.startedIds.add(utterance);
              if(lane.startedIds.size>=2048){lane.fenceOverflow=true;lane.startedIds.clear()}}
            if (played !== lane.playback) return;
            if (lane.sink) {
              if(lane.sink.chunkPlayedAt)lane.sink.chunkPlayedAt(utterance,chunk,endFrame);
              else lane.sink.chunkPlayed(utterance,chunk);
            }
            if (!lane.sources.size) { lane.playback++; closePlayback(lane, epoch, lane.queueEnd); }
          };
          source.start(at);
          const started = () => {
            if (!lane.sink || !lane.sources.has(source)) return;
            const left = at - context.currentTime;
            if (left > 0) setTimeout(started, left * 1000);
            else {if(!lane.fenceOverflow){lane.startedIds.add(utterance);
              if(lane.startedIds.size>=2048){lane.fenceOverflow=true;lane.startedIds.clear()}}
              lane.sink.chunkStarted(utterance, chunk)}
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
          lane.startedIds.clear();lane.fencedIds.clear();lane.fenceOverflow=false;
          setTimeout(() => { meter?.disconnect(); meter?.port.close(); }, FADE_SECONDS * 2000);
          if (![...lanes.values()].some((other) => other.sink)) releaseDevices();
          else if (routeWasHere && capture) void route(null).catch(() => {});
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
