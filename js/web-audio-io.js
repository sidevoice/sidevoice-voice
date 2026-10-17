// The browser's microphone and speaker for a call, `createWebAudioIo(options)`: an object with `start(sink)`,
// `play(utterance, chunk, samples, sampleRate)`, `stopPlayback()` and `stop()`, reporting to `sink` (`captured`,
// `rendered(samples)` (16 kHz output after gain/fade), `playbackEnded()` (terminal zero),
// `chunkStarted`, `chunkPlayed`, `failed`).
//
// - Capture is `getUserMedia` with the browser's echo cancellation, noise suppression and gain control: the browser's
//   canceller is the web's echo cancellation, and none other is applied. An AudioWorklet (capture-worklet.js) turns
//   it into 16 kHz mono frames of 10 ms.
// - Playback is Web Audio, on the same context: each chunk an AudioBuffer at its own rate, scheduled right after
//   what is queued. A chunk has started when the context's clock reaches its start time, and is played when its
//   source ends.
// - A microphone whose track the browser mutes (another app or the system holds it, or the device stopped delivering)
//   gives silence, not a failure: `sink.microphoneMuted(true)` says so, and `false` once it gives audio again.
// - Failures reach `sink.failed(code)` as stable codes: `microphone-denied`, `microphone-unavailable`,
//   `microphone-lost`, `audio-capture-failed` (capture worklet load/processor),
//   `audio-output-failed` (playback worklet/output). Once failed, it is stopped.
// - The microphone and the output can be chosen by id, at start (`inputDevice`, `outputDevice`) or at any time
//   (`setInputDevice`, `setOutputDevice`): a call that runs switches without ending. A chosen microphone that goes
//   away falls back to the default one, and `onDeviceLost` hears it; only the default one going away ends the call.

const FADE_SECONDS = 0.03;

/** Whether this browser lets an `AudioContext` choose its output. */
const outputSelectable = () => typeof AudioContext !== "undefined" && typeof AudioContext.prototype.setSinkId === "function";

const CAPTURE = {
  echoCancellation: true,
  noiseSuppression: true,
  autoGainControl: true,
  channelCount: 1,
};

/**
 * A microphone and speaker on Web Audio. `options.inputDevice` is the id of an audio input (`enumerateDevices`) to
 * capture from instead of the default one; `options.outputDevice` the id of an output to play on, where the browser lets
 * a context choose (`AudioContext.setSinkId`, `outputSelectable`); where it does not, playback stays on the default
 * output. Both can be changed later (`setInputDevice`, `setOutputDevice`).
 *
 * A non-default output may escape the browser's echo canceller, which takes its reference from what it plays out:
 * whether it does on each browser is an open point of sidevoice-core#89, to be checked before the web beta.
 *
 * Capture always requests browser echo cancellation. The browser supplies its own rendered output as the reference;
 * the playback worklet's rendered samples are telemetry only and never used to detect speech.
 */
export function createWebAudioIo(options = {}) {
  const { onDeviceLost } = options;
  // The chosen devices' ids; null is the system's default.
  let inputDevice = options.inputDevice || null;
  let outputDevice = options.outputDevice || null;
  let sink = null;
  let context = null;
  let stream = null;
  // The microphone's node and the worklet it feeds: a switch replaces the first and keeps the second.
  let microphone = null;
  let capture = null;
  let output = null;
  let meter = null;
  let rendering = false;
  let active = null;
  const closing = new Map();
  let deferred = null;
  let sources = new Set();
  let queueEnd = 0;
  // Each callback checks the generation it was made in, so that nothing reports after what dropped it: `session`
  // moves on at every start and stop, `playback` at every stopPlayback too.
  let session = 0;
  let playback = 0;

  // Releases the microphone, the context and the queue; returns the sink it reported to.
  function teardown() {
    session++;
    playback++;
    const old = sink;
    sink = null;
    for (const track of stream?.getTracks() ?? []) {
      track.onended = null;
      track.onmute = null;
      track.onunmute = null;
      track.stop();
    }
    stream = null;
    microphone?.disconnect();
    capture?.disconnect();
    capture?.port?.close();
    capture && (capture.port.onmessage = null);
    microphone = null;
    capture = null;
    for (const source of sources) {
      source.onended = null;
    }
    sources = new Set();
    output?.disconnect();
    meter?.disconnect();
    meter?.port?.close();
    meter && (meter.port.onmessage = null);
    meter = null;
    rendering = false;
    active = null;
    closing.clear();
    deferred = null;
    context?.close().catch(() => {});
    context = null;
    output = null;
    queueEnd = 0;
    return old;
  }

  function release(old) {
    old?.free?.();
  }

  function fail(code) {
    const old = teardown();
    if (old) {
      old.failed(code);
      release(old);
    }
  }

  // A gain node to the destination, through which every chunk plays; replaced after each fade-out.
  function newOutput() {
    output = context.createGain();
    output.connect(meter);
  }

  const outputFrame = (time) => Math.ceil(time * (context?.sampleRate || 16000));

  // The processor's fence is ordered behind its posted frames. A later play may already be scheduled, so hold its
  // first report until the old playback's terminal zero has crossed the sink.
  function closePlayback(epoch, endTime, waitMs = 0) {
    if (!epoch || closing.has(epoch.id)) return;
    epoch.endFrame = outputFrame(endTime);
    if (active === epoch) active = null;
    rendering = false;
    closing.set(epoch.id, epoch);
    const opened = session;
    setTimeout(() => {
      if (opened === session && closing.has(epoch.id)) {
        meter.port.postMessage({ type: "fence", id: epoch.id });
      }
    }, waitMs);
  }

  /** The microphone `id` (null: the default), with the call's capture constraints. Rejects with a stable `code`. */
  async function microphoneStream(id) {
    if (!navigator.mediaDevices?.getUserMedia) {
      throw Object.assign(new Error("no getUserMedia"), { code: "microphone-unavailable" });
    }
    const audio = { ...CAPTURE, ...(id ? { deviceId: { exact: id } } : {}) };
    try {
      const media = await navigator.mediaDevices.getUserMedia({ audio });
      if (media.getAudioTracks().some((track) => track.getSettings?.().echoCancellation === false)) {
        media.getTracks().forEach((track) => track.stop());
        throw Object.assign(new Error("echo cancellation unavailable"), { code: "echo-cancellation-unavailable" });
      }
      return media;
    } catch (error) {
      if (error?.code === "echo-cancellation-unavailable") throw error;
      const denied = error?.name === "NotAllowedError" || error?.name === "SecurityError";
      throw Object.assign(new Error(error?.message || "getUserMedia"), { code: denied ? "microphone-denied" : "microphone-unavailable" });
    }
  }

  /** `media` as the microphone from now: its tracks watched, and fed to the worklet when there is one. */
  function useStream(media, current) {
    const replaced = stream !== null;
    for (const track of stream?.getTracks() ?? []) {
      track.onended = null;
      track.onmute = null;
      track.onunmute = null;
      track.stop();
    }
    stream = media;
    for (const track of stream.getAudioTracks()) {
      track.onended = () => current() && microphoneEnded();
      track.onmute = () => current() && sink.microphoneMuted(true);
      track.onunmute = () => current() && sink.microphoneMuted(false);
    }
    // Muted from the outset is said at once; another microphone says whether it gives audio.
    const muted = stream.getAudioTracks().some((track) => track.muted);
    if (muted || replaced) {
      sink.microphoneMuted(muted);
    }
    if (capture) {
      microphone?.disconnect();
      microphone = context.createMediaStreamSource(stream);
      microphone.connect(capture);
    }
  }

  // The chosen microphone went away: the default one takes over, and the page is told. The default one going away
  // ends the call.
  function microphoneEnded() {
    if (!inputDevice) return fail("microphone-lost");
    const lost = inputDevice;
    inputDevice = null;
    const opened = session;
    microphoneStream(null).then((media) => {
      if (opened !== session) return media.getTracks().forEach((track) => track.stop());
      useStream(media, () => opened === session);
      onDeviceLost?.("input", lost);
    }, () => opened === session && fail("microphone-lost"));
  }

  async function open(opened) {
    const current = () => opened === session;
    let media;
    try {
      media = await microphoneStream(inputDevice);
    } catch (error) {
      return current() && fail(error.code);
    }
    if (!current()) {
      media.getTracks().forEach((track) => track.stop());
      return;
    }
    useStream(media, current);
    try {
      await context.audioWorklet.addModule(new URL("./capture-worklet.js", import.meta.url).href);
    } catch {
      return current() && fail("audio-capture-failed");
    }
    try {
      await context.audioWorklet.addModule(new URL("./playback-worklet.js", import.meta.url).href);
    } catch {
      return current() && fail("audio-output-failed");
    }
    if (!current()) {
      return;
    }
    microphone = context.createMediaStreamSource(stream);
    capture = new AudioWorkletNode(context, "sidevoice-capture");
    capture.port.onmessage = ({ data }) => {
      if(!current())return;
      if(data?.type==='captured' && Number.isSafeInteger(data.startFrame) && sink.capturedAt)
        sink.capturedAt(data.samples,data.startFrame);
      else sink.captured(data?.samples??data);
    };
    // A processor that throws posts nothing again: the capture is over, not quiet.
    capture.onprocessorerror = () => current() && fail("audio-capture-failed");
    // Connected through to the destination so that every browser runs it; it outputs silence.
    microphone.connect(capture).connect(context.destination);
    meter = new AudioWorkletNode(context, "sidevoice-playback", { outputChannelCount: [1] });
    meter.port.onmessage = ({ data }) => {
      if (!current()) return;
      if (data?.type === "fence") {
        if (!closing.delete(data.id)) return;
        sink.playbackEnded();
        if (closing.size === 0 && deferred && active && deferred.id === active.id) {
          sink.rendered(deferred.samples);
        }
        if (closing.size === 0) deferred = null;
        return;
      }
      if (data?.type === "reset") {
        if (active?.id === data.id) active.ready = true;
        return;
      }
      meter.port.postMessage("ack");
      const { samples, startFrame, endFrame } = data ?? {};
      if (!(samples instanceof Float32Array) || !Number.isFinite(startFrame) || !Number.isFinite(endFrame)) return;
      // A suspended main thread may receive seconds-old output after the processor has moved on.
      if (outputFrame(context.currentTime) - endFrame > (context.sampleRate || 16000) / 4) return;
      // A cancellation fade can overlap the next source. Through its end, those mixed frames still belong to the
      // closing playback and must be delivered before its zero, never deferred into the new playback.
      const epoch = [...closing.values()].find((past) => startFrame >= past.startFrame && endFrame <= past.endFrame)
        ?? (active && startFrame >= active.startFrame ? active : null);
      if (!epoch || (epoch.endFrame !== undefined && endFrame > epoch.endFrame) || (epoch === active && !epoch.ready)) return;
      if (epoch === active && closing.size > 0) deferred = { id: epoch.id, samples };
      else sink.rendered(samples);
    };
    meter.onprocessorerror = () => current() && fail("audio-output-failed");
    meter.connect(context.destination);
    newOutput();
    try {
      if (outputDevice && outputSelectable()) {
        await context.setSinkId(outputDevice);
      }
      await context.resume();
    } catch {
      return current() && fail("audio-output-failed");
    }
    if (current()) {
      // The microphone, the worklet and the speaker all work: the call listens from now.
      sink.ready();
    }
  }

  return {
    // currentTime names the render quantum's start. Mark its end so an app
    // publication in the same quantum cannot be placed before a mic sample.
    clockFrame() {return context ? Math.floor(context.currentTime*context.sampleRate)+128 : null;},
    /** Whether the output can be chosen in this browser (`AudioContext.setSinkId`). */
    get outputSelectable() {
      return outputSelectable();
    },

    /** The microphone from now: `id`, or the default one with null. A call that runs switches without ending, the old
     *  microphone kept until the new one works; one that does not uses it from its next start. Rejects with `code`. */
    async setInputDevice(id) {
      const next = id || null;
      if (!stream) {
        inputDevice = next;
        return;
      }
      const opened = session;
      const media = await microphoneStream(next);
      if (opened !== session) {
        media.getTracks().forEach((track) => track.stop());
        inputDevice = next;
        return;
      }
      inputDevice = next;
      useStream(media, () => opened === session);
    },

    /** The output from now: `id`, or the default one with null, on the running call too. Rejects
     *  `audio-output-unselectable` where the browser cannot choose one, `audio-output-failed` when it refuses it. */
    async setOutputDevice(id) {
      const next = id || null;
      if (!outputSelectable()) {
        if (next) throw Object.assign(new Error("setSinkId"), { code: "audio-output-unselectable" });
        return;
      }
      if (context) {
        try {
          await context.setSinkId(next ?? "");
          // A changed output is a boundary for queued telemetry, even if playback continues on the new device.
          const boundary = outputFrame(context.currentTime);
          if (active) {
            active.startFrame = boundary;
            active.ready = false;
            meter.port.postMessage({ type: "reset", id: active.id, frame: boundary });
          }
          for (const past of closing.values()) past.startFrame = Math.max(past.startFrame, boundary);
          deferred = null;
          sink?.playbackEnded();
        } catch (error) {
          throw Object.assign(new Error(error?.message || "setSinkId"), { code: "audio-output-failed" });
        }
      }
      outputDevice = next;
    },

    start(newSink) {
      release(teardown());
      try {
        context = new AudioContext();
      } catch {
        throw Object.assign(new Error("no AudioContext"), { code: "audio-output-failed" });
      }
      sink = newSink;
      open(session);
    },

    play(utterance, chunk, samples, sampleRate) {
      if (!context || !output) {
        return;
      }
      if (!rendering) {
        playback++;
        const startFrame = outputFrame(Math.max(context.currentTime, queueEnd));
        active = { id: playback, startFrame, ready: false };
        meter.port.postMessage({ type: "reset", id: active.id,
          frame: Math.max(startFrame, ...[...closing.values()].map((past) => past.endFrame)) });
        rendering = true;
      }
      const played = playback;
      const epoch = active;
      const current = () => played === playback;
      let buffer;
      try {
        // An empty chunk plays as one silent sample, so that it still starts and ends in its place.
        buffer = context.createBuffer(1, Math.max(samples.length, 1), sampleRate);
      } catch {
        return fail("audio-output-failed");
      }
      buffer.copyToChannel(samples, 0);
      const source = context.createBufferSource();
      source.buffer = buffer;
      source.connect(output);
      const startAt = Math.max(context.currentTime, queueEnd);
      queueEnd = startAt + buffer.duration;
      const endFrame = Math.ceil(queueEnd * context.sampleRate);
      sources.add(source);
      source.onended = () => {
        sources.delete(source);
        if (current()) {
          if(sink.chunkPlayedAt)sink.chunkPlayedAt(utterance, chunk, endFrame);
          else sink.chunkPlayed(utterance, chunk);
          if (sources.size === 0) {
            playback++;
            closePlayback(epoch, queueEnd);
          }
        }
      };
      source.start(startAt);
      // The context's clock decides: a timer may fire early, and then waits again for what is left.
      const started = () => {
        if (!current()) {
          return;
        }
        const left = startAt - context.currentTime;
        if (left > 0) {
          setTimeout(started, left * 1000);
        } else {
          sink.chunkStarted(utterance, chunk);
        }
      };
      started();
    },

    stopPlayback() {
      if (!context || !output) {
        return;
      }
      playback++;
      const now = context.currentTime;
      output.gain.setValueAtTime(output.gain.value, now);
      output.gain.linearRampToValueAtTime(0, now + FADE_SECONDS);
      for (const source of sources) {
        source.onended = null;
        source.stop(now + FADE_SECONDS);
      }
      sources = new Set();
      const faded = output;
      const epoch = active;
      closePlayback(epoch, now + FADE_SECONDS, FADE_SECONDS * 2000);
      setTimeout(() => {
        faded.disconnect();
      }, FADE_SECONDS * 2000);
      newOutput();
      queueEnd = 0;
    },

    stop() {
      release(teardown());
    },
  };
}
