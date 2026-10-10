// The browser's microphone and speaker for a call, `createWebAudioIo(options)`: an object with `start(sink)`,
// `play(utterance, chunk, samples, sampleRate)`, `stopPlayback()` and `stop()`, reporting to `sink` (`captured`,
// `chunkStarted`, `chunkPlayed`, `failed`).
//
// - Capture is `getUserMedia` with the browser's echo cancellation, noise suppression and gain control: the browser's
//   canceller is the web's echo cancellation, and none other is applied. An AudioWorklet (capture-worklet.js) turns
//   it into 16 kHz mono frames of 10 ms.
// - Playback is Web Audio, on the same context: each chunk an AudioBuffer at its own rate, scheduled right after
//   what is queued. A chunk has started when the context's clock reaches its start time, and is played when its
//   source ends.
// - Failures reach `sink.failed(code)` as stable codes: `microphone-denied`, `microphone-unavailable`,
//   `microphone-lost`, `audio-capture-failed`, `audio-output-failed`. Once failed, it is stopped.

const FADE_SECONDS = 0.03;

const CAPTURE = {
  echoCancellation: true,
  noiseSuppression: true,
  autoGainControl: true,
  channelCount: 1,
};

/**
 * A microphone and speaker on Web Audio. `options.outputDevice` is the id of an audio output (`enumerateDevices`)
 * to play on instead of the default one, where the browser lets a context choose (`AudioContext.setSinkId`); where it
 * does not, playback stays on the default output.
 *
 * A non-default output may escape the browser's echo canceller, which takes its reference from what it plays out:
 * whether it does on each browser is an open point of sidevoice-core#89, to be checked before the web beta.
 */
export function createWebAudioIo(options = {}) {
  const { outputDevice } = options;
  let sink = null;
  let context = null;
  let stream = null;
  let output = null;
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
      track.stop();
    }
    stream = null;
    for (const source of sources) {
      source.onended = null;
    }
    sources = new Set();
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
    output.connect(context.destination);
  }

  async function open(opened) {
    const current = () => opened === session;
    if (!navigator.mediaDevices?.getUserMedia) {
      return fail("microphone-unavailable");
    }
    let media;
    try {
      media = await navigator.mediaDevices.getUserMedia({ audio: CAPTURE });
    } catch (error) {
      const denied = error?.name === "NotAllowedError" || error?.name === "SecurityError";
      return current() && fail(denied ? "microphone-denied" : "microphone-unavailable");
    }
    if (!current()) {
      media.getTracks().forEach((track) => track.stop());
      return;
    }
    stream = media;
    for (const track of stream.getAudioTracks()) {
      track.onended = () => current() && fail("microphone-lost");
    }
    try {
      await context.audioWorklet.addModule(new URL("./capture-worklet.js", import.meta.url).href);
    } catch {
      return current() && fail("audio-capture-failed");
    }
    if (!current()) {
      return;
    }
    const microphone = context.createMediaStreamSource(stream);
    const capture = new AudioWorkletNode(context, "sidevoice-capture");
    capture.port.onmessage = ({ data }) => current() && sink.captured(data);
    // Connected through to the destination so that every browser runs it; it outputs silence.
    microphone.connect(capture).connect(context.destination);
    try {
      if (outputDevice && typeof context.setSinkId === "function") {
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
    start(newSink) {
      release(teardown());
      try {
        context = new AudioContext();
      } catch {
        throw Object.assign(new Error("no AudioContext"), { code: "audio-output-failed" });
      }
      sink = newSink;
      newOutput();
      open(session);
    },

    play(utterance, chunk, samples, sampleRate) {
      if (!context) {
        return;
      }
      const played = playback;
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
      sources.add(source);
      source.onended = () => {
        sources.delete(source);
        if (current()) {
          sink.chunkPlayed(utterance, chunk);
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
      if (!context) {
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
      setTimeout(() => faded.disconnect(), FADE_SECONDS * 2000);
      newOutput();
      queueEnd = 0;
    },

    stop() {
      release(teardown());
    },
  };
}
