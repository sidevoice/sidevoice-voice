//! The device's own microphone and speaker, natively ([`NativeIo`]): the system's default input and output through
//! cpal, with WebRTC's AEC3 between them. cpal and the canceller are native libraries; the browser has its own
//! microphone and speaker in the page.
//!
//! The device callbacks only move samples through lock-free rings: the input's into the capture ring, and the
//! output's from the playback ring, copying what it plays into the reference ring (`output`). One audio thread of
//! the module owns the devices' streams, takes both rings in step through the echo canceller (`pipeline`), sends the
//! clean 16 kHz frames to the call, feeds the chunks it is handed into the playback ring as room frees up, and tells
//! the call when each chunk starts and ends at the speaker. No C++ runs in a device callback.
#![cfg(native)]

mod output;
mod pipeline;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};

use self::output::{Counters, Output, Queue};
use self::pipeline::Pipeline;
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

/// The system's default microphone and speaker, with echo cancellation.
#[derive(Default)]
pub struct NativeIo {
    running: Option<Running>,
}

/// An open microphone and speaker.
struct Running {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    speaker: mpsc::Sender<Command>,
    output_rate: u32,
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

/// What the audio thread hands back once the devices are open: the speaker's rate.
type Opened = Result<u32, String>;

impl NativeIo {
    /// The default devices, opened at `start`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl AudioIo for NativeIo {
    fn start(&mut self, sink: IoSink) -> Result<(), String> {
        self.stop();
        let stop = Arc::new(AtomicBool::new(false));
        let (opened, open) = mpsc::channel::<Opened>();
        let (speaker, commands) = mpsc::channel::<Command>();
        let thread = {
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name("sidevoice-voice-audio".into())
                .spawn(move || run(&sink, &stop, &commands, &opened))
                .map_err(|_| "audio-thread-failed".to_owned())?
        };
        let output_rate = open
            .recv()
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
        Resampler::new(sample_rate, running.output_rate).process(&samples, &mut resampled);
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

/// The audio thread: opens the devices, then moves audio until `stop`.
fn run(
    sink: &IoSink,
    stop: &AtomicBool,
    commands: &mpsc::Receiver<Command>,
    opened: &mpsc::Sender<Opened>,
) {
    let changed = Arc::new(AtomicBool::new(false));
    let counters = Arc::new(Counters::default());
    let devices = match Devices::open(sink, &counters, &changed) {
        Ok(devices) => devices,
        Err(code) => {
            let _ = opened.send(Err(code));
            return;
        }
    };
    let Devices {
        streams: _streams,
        mut capture,
        mut reference,
        mut pipeline,
        mut playback,
        output_rate,
        channels,
    } = devices;
    let _ = opened.send(Ok(output_rate));
    let mut queue = Queue::default();
    let mut heard = Vec::new();
    let mut played = Vec::new();
    while !stop.load(Ordering::Acquire) {
        for command in commands.try_iter() {
            match command {
                Command::Play {
                    utterance,
                    chunk,
                    samples,
                } => queue.play(utterance, chunk, samples),
                Command::Stop => queue.stop(&counters),
            }
        }
        queue.feed(&mut playback);
        drain(&mut reference, &mut played, 1);
        drain(&mut capture, &mut heard, channels);
        if changed.swap(false, Ordering::AcqRel) {
            pipeline.reinitialize();
        }
        pipeline.render(&played);
        for frame in pipeline.capture(&heard) {
            sink.send(IoEvent::Captured(frame.to_vec()));
        }
        let consumed = counters.consumed.load(Ordering::Acquire);
        for event in queue.schedule.due(consumed) {
            sink.send(event);
        }
        thread::sleep(TICK);
    }
}

/// The open devices and what the audio thread keeps of them.
struct Devices {
    streams: [Stream; 2],
    capture: Consumer<f32>,
    reference: Consumer<f32>,
    pipeline: Pipeline,
    playback: Producer<f32>,
    output_rate: u32,
    /// The microphone's channels, interleaved in the capture ring.
    channels: usize,
}

impl Devices {
    fn open(
        sink: &IoSink,
        counters: &Arc<Counters>,
        changed: &Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let host = cpal::default_host();
        let input = host
            .default_input_device()
            .ok_or("microphone-unavailable")?;
        let output = host.default_output_device().ok_or("speaker-unavailable")?;
        let input_config = input.default_input_config().map_err(|error| code(&error))?;
        let output_config = output
            .default_output_config()
            .map_err(|error| code(&error))?;
        let (input_rate, channels) = (
            input_config.sample_rate(),
            usize::from(input_config.channels()),
        );
        let output_rate = output_config.sample_rate();

        let (capture_in, capture) =
            RingBuffer::new((input_rate * BACKLOG_SECONDS) as usize * channels);
        let (reference_in, reference) = RingBuffer::new((output_rate * BACKLOG_SECONDS) as usize);
        let (playback, playback_out) = RingBuffer::new((output_rate * PLAYBACK_SECONDS) as usize);
        let fade = (output_rate * FADE_MS / 1000) as usize;
        let player = Output::new(playback_out, reference_in, fade);
        let pipeline = Pipeline::new(input_rate, channels, output_rate)?;

        let microphone = match input_config.sample_format() {
            SampleFormat::F32 => {
                listen::<f32>(&input, input_config.config(), capture_in, sink, changed)
            }
            SampleFormat::I16 => {
                listen::<i16>(&input, input_config.config(), capture_in, sink, changed)
            }
            SampleFormat::U16 => {
                listen::<u16>(&input, input_config.config(), capture_in, sink, changed)
            }
            SampleFormat::I32 => {
                listen::<i32>(&input, input_config.config(), capture_in, sink, changed)
            }
            _ => Err("audio-format-unsupported".into()),
        }?;
        let config = output_config.config();
        let speaker = match output_config.sample_format() {
            SampleFormat::F32 => speak::<f32>(&output, config, player, counters, sink, changed),
            SampleFormat::I16 => speak::<i16>(&output, config, player, counters, sink, changed),
            SampleFormat::U16 => speak::<u16>(&output, config, player, counters, sink, changed),
            SampleFormat::I32 => speak::<i32>(&output, config, player, counters, sink, changed),
            _ => Err("audio-format-unsupported".into()),
        }?;
        microphone.play().map_err(|error| code(&error))?;
        speaker.play().map_err(|error| code(&error))?;
        Ok(Self {
            streams: [microphone, speaker],
            capture,
            reference,
            pipeline,
            playback,
            output_rate,
            channels,
        })
    }
}

/// The microphone's stream: every sample into the capture ring.
fn listen<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut ring: Producer<f32>,
    sink: &IoSink,
    changed: &Arc<AtomicBool>,
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
            failure(sink, changed),
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
    sink: &IoSink,
    changed: &Arc<AtomicBool>,
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
            failure(sink, changed),
            None,
        )
        .map_err(|error| code(&error))
}

/// What a stream does with its errors: a device that changed under it starts the canceller over; any other failure
/// is told once, and the call stops.
fn failure(sink: &IoSink, changed: &Arc<AtomicBool>) -> impl FnMut(cpal::Error) + Send + 'static {
    let (sink, changed) = (sink.clone(), Arc::clone(changed));
    let mut told = false;
    move |error| {
        if error.kind() == ErrorKind::DeviceChanged {
            changed.store(true, Ordering::Release);
        } else if !told {
            told = true;
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
