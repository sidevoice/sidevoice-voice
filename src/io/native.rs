//! The device's own microphone and speaker, natively ([`NativeIo`]): the system's input and output through cpal —
//! the default ones, or those chosen ([`NativeDevices`], switched while the call runs) — with WebRTC's AEC3 between
//! them. cpal and the canceller are native libraries; the browser has its own
//! microphone and speaker in the page.
//!
//! The device callbacks only move samples through lock-free rings: the input's into the capture ring, and the
//! output's from the playback ring, copying what it plays into the reference ring (`output`). One audio thread of
//! the module owns the devices' streams, takes both rings in step through the echo canceller (`pipeline`), sends the
//! clean 16 kHz frames to the call, feeds the chunks it is handed into the playback ring as room frees up, and tells
//! the call when each chunk starts and ends at the speaker. No C++ runs in a device callback.
#![cfg(native)]

mod devices;
mod output;
mod pipeline;
mod shared;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};

use self::devices::{by_id, Kind};
pub use self::devices::{AudioDevice, AudioDevices, LostDevices, NativeDevices};
use self::output::{Counters, Output, Queue, Schedule};
use self::pipeline::Pipeline;
pub use self::shared::{InputPort, SharedNativeAudio};
use crate::audio::Resampler;
use crate::io::{AudioIo, IoEvent, IoSink};

/// How long a stop fades out, in milliseconds.
const FADE_MS: u32 = 30;
/// How much audio the playback ring holds, in seconds.
const PLAYBACK_SECONDS: u32 = 120;
/// How much unread capture and reference the rings hold, in seconds.
const BACKLOG_SECONDS: u32 = 2;
/// How often the audio thread wakes.
const TICK: Duration = Duration::from_millis(5);

/// The system's microphone and speaker, the default ones or those chosen, with echo cancellation.
#[derive(Default)]
pub struct NativeIo {
    devices: NativeDevices,
    running: Option<Running>,
}

/// An open microphone and speaker.
struct Running {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    speaker: mpsc::Sender<Command>,
    /// The speaker's rate: a switch to another speaker may change it.
    output_rate: Arc<AtomicU32>,
}

/// What the audio thread is told to do with the speaker, in order.
enum Command {
    /// Queue a chunk, at the output's rate.
    Play {
        utterance: String,
        chunk: usize,
        samples: Vec<f32>,
    },
    /// Stop with a fade and drop what is queued.
    Stop,
}

/// What the audio thread hands back once the devices are open, or why they are not.
type Opened = Result<(), String>;

impl NativeIo {
    /// The default devices, opened at `start`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The devices `devices` chooses, opened at `start` and switched when it chooses others.
    #[must_use]
    pub fn with_devices(devices: NativeDevices) -> Self {
        Self {
            devices,
            running: None,
        }
    }
}

impl AudioIo for NativeIo {
    fn start(&mut self, sink: IoSink) -> Result<(), String> {
        self.stop();
        let stop = Arc::new(AtomicBool::new(false));
        let (opened, open) = mpsc::channel::<Opened>();
        let (speaker, commands) = mpsc::channel::<Command>();
        let output_rate = Arc::new(AtomicU32::new(0));
        let thread = {
            let (stop, rate, devices) = (
                Arc::clone(&stop),
                Arc::clone(&output_rate),
                self.devices.clone(),
            );
            thread::Builder::new()
                .name("sidevoice-voice-audio".into())
                .spawn(move || run(&sink, &devices, &rate, &stop, &commands, &opened))
                .map_err(|_| "audio-thread-failed".to_owned())?
        };
        open.recv()
            .unwrap_or_else(|_| Err("audio-thread-failed".into()))?;
        self.running = Some(Running {
            stop,
            thread: Some(thread),
            speaker,
            output_rate,
        });
        Ok(())
    }

    fn play(&mut self, utterance: &str, chunk: usize, samples: Vec<f32>, sample_rate: u32) {
        let Some(running) = &mut self.running else {
            return;
        };
        let mut resampled = Vec::with_capacity(samples.len());
        let output_rate = running.output_rate.load(Ordering::Acquire);
        Resampler::new(sample_rate, output_rate).process(&samples, &mut resampled);
        let _ = running.speaker.send(Command::Play {
            utterance: utterance.to_owned(),
            chunk,
            samples: resampled,
        });
    }

    fn stop_playback(&mut self) {
        if let Some(running) = &self.running {
            let _ = running.speaker.send(Command::Stop);
        }
    }

    fn stop(&mut self) {
        if let Some(mut running) = self.running.take() {
            running.stop.store(true, Ordering::Release);
            if let Some(thread) = running.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

impl Drop for NativeIo {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The audio thread: opens the devices, then moves audio until `stop`. A microphone chosen anew is switched at once;
/// a speaker once nothing is left to play, so that no chunk the call waits on is lost with the old one.
fn run(
    sink: &IoSink,
    devices: &NativeDevices,
    output_rate: &AtomicU32,
    stop: &AtomicBool,
    commands: &mpsc::Receiver<Command>,
    opened: &mpsc::Sender<Opened>,
) {
    let changed = Arc::new(AtomicBool::new(false));
    let host = cpal::default_host();
    // What is chosen from here on is switched to.
    devices.take_switch(Kind::Input);
    devices.take_switch(Kind::Output);
    let choice = devices.chosen();
    let opening = Microphone::open(&host, choice.input.as_deref(), sink, devices, &changed)
        .and_then(|microphone| {
            let speaker = Speaker::open(&host, choice.output.as_deref(), sink, devices, &changed)?;
            let pipeline = Pipeline::new(microphone.rate, microphone.channels, speaker.rate)?;
            Ok((microphone, speaker, pipeline))
        });
    let (mut microphone, mut speaker, mut pipeline) = match opening {
        Ok(open) => open,
        Err(code) => {
            let _ = opened.send(Err(code));
            return;
        }
    };
    output_rate.store(speaker.rate, Ordering::Release);
    let _ = opened.send(Ok(()));
    // Both streams play: the call listens from now.
    sink.send(IoEvent::Ready);
    let mut queue = Queue::default();
    let mut heard = Vec::new();
    let mut played = Vec::new();
    let mut speaker_switch = false;
    let mut ending: Option<u64> = None;
    let mut had_chunks = false;
    let mut drained_reference = 0_u64;
    while !stop.load(Ordering::Acquire) {
        for command in commands.try_iter() {
            match command {
                Command::Play {
                    utterance,
                    chunk,
                    samples,
                } => {
                    had_chunks = true;
                    ending = None;
                    queue.play(utterance, chunk, samples);
                }
                Command::Stop => ending = Some(queue.stop(&speaker.counters)),
            }
        }
        if devices.take_switch(Kind::Input) {
            match switched(
                Microphone::open(
                    &host,
                    devices.chosen().input.as_deref(),
                    sink,
                    devices,
                    &changed,
                ),
                &speaker,
            ) {
                Ok((next, rebuilt)) => (microphone, pipeline) = (next, rebuilt),
                Err(code) => return sink.send(IoEvent::Failed(code)),
            }
        }
        speaker_switch |= devices.take_switch(Kind::Output);
        // A speaker that went away plays nothing more: what it was left to play is over.
        let gone = speaker.gone.load(Ordering::Acquire);
        if speaker_switch && (queue.is_idle() || gone) {
            speaker_switch = false;
            for event in queue.abandon() {
                sink.send(event);
            }
            let reopened = Speaker::open(
                &host,
                devices.chosen().output.as_deref(),
                sink,
                devices,
                &changed,
            )
            .and_then(|next| {
                Ok((
                    Pipeline::new(microphone.rate, microphone.channels, next.rate)?,
                    next,
                ))
            });
            match reopened {
                Ok((rebuilt, next)) => {
                    (speaker, pipeline, queue) = (next, rebuilt, Queue::default());
                    drained_reference = 0;
                    sink.send(IoEvent::PlaybackEnded);
                    ending = None;
                    had_chunks = false;
                    output_rate.store(speaker.rate, Ordering::Release);
                }
                Err(code) => return sink.send(IoEvent::Failed(code)),
            }
        }
        queue.feed(&mut speaker.playback);
        drain(&mut speaker.reference, &mut played, 1);
        drained_reference += played.len() as u64;
        drain(&mut microphone.capture, &mut heard, microphone.channels);
        if changed.swap(false, Ordering::AcqRel) {
            pipeline.reinitialize();
        }
        let rendered = pipeline.render(&played);
        if (had_chunks || ending.is_some()) && !rendered.is_empty() {
            sink.send(IoEvent::Rendered(rendered));
        }
        let frames = pipeline.capture(&heard);
        let consumed = speaker.counters.consumed.load(Ordering::Acquire);
        // The callback publishes its consumed count after writing the reference. Report every playback boundary
        // visible at this snapshot before the capture from this tick can qualify a turn and freeze its extent.
        send_capture_after_playback(
            &mut queue.schedule,
            consumed,
            frames.into_iter().map(|frame| frame.to_vec()),
            |event| sink.send(event),
        );
        // `consumed` is published after the callback's reference writes. A callback may run between the drain above
        // and this load; wait until its published reference tail has crossed the worker before ending the meter.
        if playback_complete(
            ending,
            had_chunks,
            queue.is_idle(),
            consumed,
            speaker.counters.referenced.load(Ordering::Acquire),
            drained_reference,
        ) {
            sink.send(IoEvent::PlaybackEnded);
            pipeline.reset_playback_meter();
            ending = None;
            had_chunks = false;
        }
        thread::sleep(TICK);
    }
}

fn send_capture_after_playback(
    schedule: &mut Schedule,
    consumed: u64,
    frames: impl IntoIterator<Item = Vec<f32>>,
    mut send: impl FnMut(IoEvent),
) {
    for event in schedule.due(consumed) {
        send(event);
    }
    for frame in frames {
        send(IoEvent::Captured(frame));
    }
}

/// Completion is behind the callback's published reference tail, even if it ran after this tick's drain.
fn playback_complete(
    ending: Option<u64>,
    had_chunks: bool,
    idle: bool,
    consumed: u64,
    referenced: u64,
    drained_reference: u64,
) -> bool {
    (ending.is_some_and(|end| consumed >= end) || (had_chunks && idle && ending.is_none()))
        && drained_reference >= referenced
}

/// A new microphone, and the echo canceller made again for its rate and the speaker's.
fn switched(
    microphone: Result<Microphone, String>,
    speaker: &Speaker,
) -> Result<(Microphone, Pipeline), String> {
    let microphone = microphone?;
    let pipeline = Pipeline::new(microphone.rate, microphone.channels, speaker.rate)?;
    Ok((microphone, pipeline))
}

/// The device of `kind` with `id` (the default one with none). A chosen one that is not there any more is lost, and
/// the default one is opened instead.
fn device(
    host: &cpal::Host,
    kind: Kind,
    id: Option<&str>,
    devices: &NativeDevices,
) -> Result<cpal::Device, String> {
    if let Some(id) = id {
        if let Some(device) = by_id(host, id) {
            return Ok(device);
        }
        devices.lose(kind);
    }
    match kind {
        Kind::Input => host
            .default_input_device()
            .ok_or_else(|| "microphone-unavailable".into()),
        Kind::Output => host
            .default_output_device()
            .ok_or_else(|| "speaker-unavailable".into()),
    }
}

/// An open microphone: its stream, what it captures and at which rate.
struct Microphone {
    _stream: Stream,
    capture: Consumer<f32>,
    rate: u32,
    /// Its channels, interleaved in the capture ring.
    channels: usize,
}

impl Microphone {
    fn open(
        host: &cpal::Host,
        id: Option<&str>,
        sink: &IoSink,
        devices: &NativeDevices,
        changed: &Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let input = device(host, Kind::Input, id, devices)?;
        let config = input.default_input_config().map_err(|error| code(&error))?;
        let (rate, channels) = (config.sample_rate(), usize::from(config.channels()));
        let (capture_in, capture) = RingBuffer::new((rate * BACKLOG_SECONDS) as usize * channels);
        let failed = failure(
            sink,
            changed,
            id.map(|_| (Kind::Input, devices)),
            &Arc::default(),
        );
        let stream = match config.sample_format() {
            SampleFormat::F32 => listen::<f32>(&input, config.config(), capture_in, failed),
            SampleFormat::I16 => listen::<i16>(&input, config.config(), capture_in, failed),
            SampleFormat::U16 => listen::<u16>(&input, config.config(), capture_in, failed),
            SampleFormat::I32 => listen::<i32>(&input, config.config(), capture_in, failed),
            _ => Err("audio-format-unsupported".into()),
        }?;
        stream.play().map_err(|error| code(&error))?;
        Ok(Self {
            _stream: stream,
            capture,
            rate,
            channels,
        })
    }
}

/// An open speaker: its stream, the rings it plays from and copies what it plays to, its counters and its rate.
struct Speaker {
    _stream: Stream,
    playback: Producer<f32>,
    reference: Consumer<f32>,
    counters: Arc<Counters>,
    rate: u32,
    /// Whether the chosen device went away under it.
    gone: Arc<AtomicBool>,
}

impl Speaker {
    fn open(
        host: &cpal::Host,
        id: Option<&str>,
        sink: &IoSink,
        devices: &NativeDevices,
        changed: &Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let output = device(host, Kind::Output, id, devices)?;
        let config = output
            .default_output_config()
            .map_err(|error| code(&error))?;
        let rate = config.sample_rate();
        let (reference_in, reference) = RingBuffer::new((rate * BACKLOG_SECONDS) as usize);
        let (playback, playback_out) = RingBuffer::new((rate * PLAYBACK_SECONDS) as usize);
        let player = Output::new(playback_out, reference_in, (rate * FADE_MS / 1000) as usize);
        let counters = Arc::new(Counters::default());
        let gone = Arc::new(AtomicBool::new(false));
        let failed = failure(sink, changed, id.map(|_| (Kind::Output, devices)), &gone);
        let stream_config = config.config();
        let stream = match config.sample_format() {
            SampleFormat::F32 => speak::<f32>(&output, stream_config, player, &counters, failed),
            SampleFormat::I16 => speak::<i16>(&output, stream_config, player, &counters, failed),
            SampleFormat::U16 => speak::<u16>(&output, stream_config, player, &counters, failed),
            SampleFormat::I32 => speak::<i32>(&output, stream_config, player, &counters, failed),
            _ => Err("audio-format-unsupported".into()),
        }?;
        stream.play().map_err(|error| code(&error))?;
        Ok(Self {
            _stream: stream,
            playback,
            reference,
            counters,
            rate,
            gone,
        })
    }
}

/// The microphone's stream: every sample into the capture ring.
fn listen<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut ring: Producer<f32>,
    failed: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    device
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                for &sample in data {
                    let _ = ring.push(sample.to_sample::<f32>());
                }
            },
            failed,
            None,
        )
        .map_err(|error| code(&error))
}

/// The speaker's stream: every frame from the player, the same sample on every channel.
fn speak<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut player: Output,
    counters: &Arc<Counters>,
    failed: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels).max(1);
    let counters = Arc::clone(counters);
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _| {
                for frame in data.chunks_exact_mut(channels) {
                    let sample = T::from_sample(player.next(&counters));
                    frame.fill(sample);
                }
                player.publish(&counters);
            },
            failed,
            None,
        )
        .map_err(|error| code(&error))
}

/// What a stream does with its errors: a device that changed under it starts the canceller over; a chosen device
/// (`chosen`: its kind, and the choice) that went away is lost and marked `gone`, and the default one takes over; any
/// other failure is told once, and the call stops.
fn failure(
    sink: &IoSink,
    changed: &Arc<AtomicBool>,
    chosen: Option<(Kind, &NativeDevices)>,
    gone: &Arc<AtomicBool>,
) -> impl FnMut(cpal::Error) + Send + 'static {
    let (sink, changed, gone) = (sink.clone(), Arc::clone(changed), Arc::clone(gone));
    let chosen = chosen.map(|(kind, devices)| (kind, devices.clone()));
    let mut told = false;
    move |error| {
        if error.kind() == ErrorKind::DeviceChanged {
            changed.store(true, Ordering::Release);
            return;
        }
        if told {
            return;
        }
        told = true;
        let lost = error.kind() == ErrorKind::DeviceNotAvailable
            && chosen
                .as_ref()
                .is_some_and(|(kind, devices)| devices.lose(*kind));
        if lost {
            gone.store(true, Ordering::Release);
        } else {
            sink.send(IoEvent::Failed(code(&error)));
        }
    }
}

/// Moves what `ring` holds in whole frames of `channels` interleaved samples into `out`, after clearing it. A frame
/// the callback has only begun to write stays in the ring for the next time.
fn drain(ring: &mut Consumer<f32>, out: &mut Vec<f32>, channels: usize) {
    out.clear();
    let slots = ring.slots();
    if let Ok(chunk) = ring.read_chunk(slots - slots % channels.max(1)) {
        let (first, second) = chunk.as_slices();
        out.extend_from_slice(first);
        out.extend_from_slice(second);
        chunk.commit_all();
    }
}

/// The stable code of a device failure.
fn code(error: &cpal::Error) -> String {
    match error.kind() {
        ErrorKind::PermissionDenied => "microphone-denied",
        ErrorKind::DeviceNotAvailable => "audio-device-unavailable",
        _ => "audio-device-failed",
    }
    .into()
}
