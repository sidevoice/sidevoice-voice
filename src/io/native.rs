//! The device's own microphone and speaker, natively ([`NativeIo`]): the system's input and output through cpal —
//! the default ones, or those chosen ([`NativeDevices`], switched while the call runs) — with WebRTC's AEC3 between
//! them. cpal and the canceller are native libraries; the browser has its own
//! microphone and speaker in the page.
//!
//! The device callbacks only move samples through lock-free rings: the input's into the capture ring, and the
//! output's from the playback ring, copying what it plays into the reference ring (`output`). One audio thread of
//! the module owns the devices' streams, takes both rings in step through the echo canceller (`pipeline`), sends the
//! clean 16 kHz frames to the call, resamples the chunks it is handed to the speaker's rate and feeds them into the
//! playback ring as room frees up (`playout`), and tells the call when each chunk starts and ends at the speaker. No
//! C++ runs in a device callback.
#![cfg(native)]

mod devices;
mod output;
mod pipeline;
mod playout;
mod shared;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};

use self::devices::{by_id, Kind};
pub use self::devices::{AudioDevice, AudioDevices, LostDevices, NativeDevices};
use self::output::{Counters, Output, ReferenceSample};
use self::pipeline::Pipeline;
use self::playout::{Command, Playout};
pub use self::shared::{InputPort, SharedNativeAudio};
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
    /// Whether the microphone should be open, each time that changes.
    microphone: mpsc::Sender<bool>,
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
    fn start(&mut self, sink: IoSink, input: bool) -> Result<(), String> {
        self.stop();
        let stop = Arc::new(AtomicBool::new(false));
        let (opened, open) = mpsc::channel::<Opened>();
        let (speaker, commands) = mpsc::channel::<Command>();
        let (microphone, wanted) = mpsc::channel::<bool>();
        let thread = {
            let (stop, devices) = (Arc::clone(&stop), self.devices.clone());
            let control = Control {
                stop,
                commands,
                wanted,
                opened,
            };
            thread::Builder::new()
                .name("sidevoice-voice-audio".into())
                .spawn(move || run(&sink, &devices, input, &control))
                .map_err(|_| "audio-thread-failed".to_owned())?
        };
        open.recv()
            .unwrap_or_else(|_| Err("audio-thread-failed".into()))?;
        self.running = Some(Running {
            stop,
            thread: Some(thread),
            speaker,
            microphone,
        });
        Ok(())
    }

    fn set_input(&mut self, input: bool) {
        if let Some(running) = &self.running {
            let _ = running.microphone.send(input);
        }
    }

    fn play(&mut self, utterance: &str, chunk: usize, samples: Vec<f32>, sample_rate: u32) {
        let Some(running) = &mut self.running else {
            return;
        };
        let _ = running.speaker.send(Command::Play {
            utterance: utterance.to_owned(),
            chunk,
            samples,
            sample_rate,
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

/// The audio thread: opens the speaker and, with `input`, the microphone, then moves audio until `stop`. A microphone
/// chosen anew is switched at once; a speaker once nothing is left to play, so that no chunk the call waits on is lost
/// with the old one. Without a microphone the echo canceller still takes the speaker's reference, and hears nothing.
/// What the audio thread is told, and where it says the devices opened.
struct Control {
    stop: Arc<AtomicBool>,
    commands: mpsc::Receiver<Command>,
    /// Whether the microphone should be open.
    wanted: mpsc::Receiver<bool>,
    opened: mpsc::Sender<Opened>,
}

fn run(sink: &IoSink, devices: &NativeDevices, input: bool, control: &Control) {
    let Control {
        stop,
        commands,
        wanted,
        opened,
    } = control;
    let changed = Arc::new(AtomicBool::new(false));
    let host = cpal::default_host();
    // What is chosen from here on is switched to.
    devices.take_switch(Kind::Input);
    devices.take_switch(Kind::Output);
    let choice = devices.chosen();
    // A microphone that cannot open is said lost, before the devices are ready: the speaker opens all the same.
    let microphone =
        input.then(|| Microphone::open(&host, choice.input.as_deref(), sink, devices, &changed));
    let microphone = match microphone {
        Some(Err(code)) => {
            sink.send(IoEvent::InputLost(code));
            None
        }
        Some(Ok(microphone)) => Some(microphone),
        None => None,
    };
    let opening = Speaker::open(&host, choice.output.as_deref(), sink, devices, &changed).and_then(
        |speaker| {
            let pipeline = pipeline_for(microphone.as_ref(), &speaker)?;
            Ok((microphone, speaker, pipeline))
        },
    );
    let (mut microphone, mut speaker, mut pipeline) = match opening {
        Ok(open) => open,
        Err(code) => {
            let _ = opened.send(Err(code));
            return;
        }
    };
    let _ = opened.send(Ok(()));
    // Both streams play: the call listens from now.
    sink.send(IoEvent::Ready);
    let mut playout = Playout::new(speaker.rate);
    let mut heard = Vec::new();
    let mut speaker_switch = false;
    while !stop.load(Ordering::Acquire) {
        for command in commands.try_iter() {
            playout.command(command, &speaker.counters);
        }
        // The microphone opened or closed as asked; one that failed under the call is closed (it said so).
        let lost = microphone
            .as_ref()
            .is_some_and(|open| open.failed.load(Ordering::Acquire));
        let want = wanted.try_iter().last();
        let close = lost || (want == Some(false) && microphone.is_some());
        let open = want == Some(true) && (microphone.is_none() || lost);
        if close || open {
            let next = if open {
                match Microphone::open(
                    &host,
                    devices.chosen().input.as_deref(),
                    sink,
                    devices,
                    &changed,
                ) {
                    Ok(next) => Some(next),
                    Err(code) => {
                        sink.send(IoEvent::InputLost(code));
                        None
                    }
                }
            } else {
                None
            };
            match pipeline_for(next.as_ref(), &speaker) {
                Ok(rebuilt) => (microphone, pipeline) = (next, rebuilt),
                Err(code) => return sink.send(IoEvent::Failed(code)),
            }
        }
        if devices.take_switch(Kind::Input) && microphone.is_some() {
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
                Ok((next, rebuilt)) => (microphone, pipeline) = (Some(next), rebuilt),
                Err(code) => {
                    sink.send(IoEvent::InputLost(code));
                    match pipeline_for(None, &speaker) {
                        Ok(rebuilt) => (microphone, pipeline) = (None, rebuilt),
                        Err(code) => return sink.send(IoEvent::Failed(code)),
                    }
                }
            }
        }
        speaker_switch |= devices.take_switch(Kind::Output);
        // A speaker that went away plays nothing more: what it was left to play is over.
        let gone = speaker.gone.load(Ordering::Acquire);
        if speaker_switch && (playout.queue.is_idle() || gone) {
            speaker_switch = false;
            for event in playout.queue.abandon() {
                sink.send(event);
            }
            let reopened = Speaker::open(
                &host,
                devices.chosen().output.as_deref(),
                sink,
                devices,
                &changed,
            )
            .and_then(|next| Ok((pipeline_for(microphone.as_ref(), &next)?, next)));
            match reopened {
                Ok((rebuilt, next)) => {
                    (speaker, pipeline) = (next, rebuilt);
                    playout = Playout::new(speaker.rate);
                    sink.send(IoEvent::PlaybackEnded);
                }
                Err(code) => return sink.send(IoEvent::Failed(code)),
            }
        }
        playout.exchange(&mut speaker.playback, &mut speaker.reference);
        match &mut microphone {
            Some(microphone) => drain(&mut microphone.capture, &mut heard, microphone.channels),
            None => heard.clear(),
        }
        if changed.swap(false, Ordering::AcqRel) {
            pipeline.reinitialize();
        }
        playout.render(&mut pipeline, sink);
        let frames = pipeline.capture(&heard);
        playout.report(
            &speaker.counters,
            frames.into_iter().map(|frame| frame.to_vec()),
            &mut pipeline,
            sink,
        );
        thread::sleep(TICK);
    }
}

/// The echo canceller for `microphone`'s rate and channels and `speaker`'s rate; without a microphone, for 16 kHz mono
/// capture that never comes.
fn pipeline_for(microphone: Option<&Microphone>, speaker: &Speaker) -> Result<Pipeline, String> {
    let (rate, channels) = microphone.map_or((16_000, 1), |microphone| {
        (microphone.rate, microphone.channels)
    });
    Pipeline::new(rate, channels, speaker.rate)
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
    /// Whether it failed under the call (told as [`IoEvent::InputLost`]), or the chosen one went away (the default
    /// one is switched to).
    failed: Arc<AtomicBool>,
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
        let failed_flag = Arc::new(AtomicBool::new(false));
        let failed = failure(
            sink,
            changed,
            id.map(|_| (Kind::Input, devices)),
            &failed_flag,
            true,
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
            failed: failed_flag,
        })
    }
}

/// An open speaker: its stream, the rings it plays from and copies what it plays to, its counters and its rate.
struct Speaker {
    _stream: Stream,
    playback: Producer<f32>,
    reference: Consumer<ReferenceSample>,
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
        let failed = failure(
            sink,
            changed,
            id.map(|_| (Kind::Output, devices)),
            &gone,
            false,
        );
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
    input: bool,
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
        // A chosen device that went away gives way to the default one (the speaker once it is marked `gone`, the
        // microphone through the switch its loss made). A microphone that fails otherwise is closed and said lost; a
        // speaker that fails ends the call.
        if lost {
            if !input {
                gone.store(true, Ordering::Release);
            }
        } else if input {
            gone.store(true, Ordering::Release);
            sink.send(IoEvent::InputLost(code(&error)));
        } else {
            sink.send(IoEvent::Failed(code(&error)));
        }
    }
}

/// Moves what `ring` holds in whole frames of `channels` interleaved samples into `out`, after clearing it. A frame
/// the callback has only begun to write stays in the ring for the next time.
fn drain<T: Clone>(ring: &mut Consumer<T>, out: &mut Vec<T>, channels: usize) {
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
