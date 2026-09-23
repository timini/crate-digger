//! Audio playback.
//!
//! A decoder thread decodes, converts to stereo, resamples to the output
//! rate and fills a lock-free ring buffer holding about two seconds of
//! audio. The output callback only copies samples out of the ring, applies
//! volume and counts underruns, so heavy background work (analysis, import)
//! has two seconds of slack before playback can glitch.
//!
//! The output is either the default device (cpal) or a null output that
//! consumes audio at real-time pace, used by tests and headless CI.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rtrb::{Consumer, Producer, RingBuffer};
use serde::Serialize;

use crate::decode::{AudioError, AudioInfo, Decoder};
use crate::resample::StereoResampler;

/// Samples in the ring are interleaved stereo.
const RING_CHANNELS: usize = 2;
const RING_SECONDS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayState {
    Idle,
    Playing,
    Paused,
    Ended,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlayerStatus {
    pub state: PlayState,
    pub path: Option<PathBuf>,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub volume: f32,
    pub error: Option<String>,
    /// Times the output needed audio that was not ready.
    pub underruns: u64,
    pub output: String,
}

#[derive(Debug, Clone)]
pub enum OutputConfig {
    /// The system's default output device.
    Device,
    /// No sound; consumes audio at real-time pace. For tests and CI.
    Null {
        sample_rate: u32,
        channels: u16,
        period_frames: usize,
    },
}

#[derive(Default)]
struct Meta {
    path: Option<PathBuf>,
    duration_ms: Option<u64>,
    error: Option<String>,
    loaded: bool,
    output: String,
}

struct Shared {
    playing: AtomicBool,
    volume_bits: AtomicU32,
    /// Bumped by the decoder to ask the output to discard queued audio.
    generation: AtomicU64,
    ack_generation: AtomicU64,
    frames_played: AtomicU64,
    base_ms: AtomicU64,
    decoder_done: AtomicBool,
    ended: AtomicBool,
    underruns: AtomicU64,
    out_rate: AtomicU32,
    meta: Mutex<Meta>,
}

impl Shared {
    fn new() -> Self {
        Shared {
            playing: AtomicBool::new(false),
            volume_bits: AtomicU32::new(1.0f32.to_bits()),
            generation: AtomicU64::new(0),
            ack_generation: AtomicU64::new(0),
            frames_played: AtomicU64::new(0),
            base_ms: AtomicU64::new(0),
            decoder_done: AtomicBool::new(true),
            ended: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
            out_rate: AtomicU32::new(48_000),
            meta: Mutex::new(Meta::default()),
        }
    }

    fn position_ms(&self) -> u64 {
        let rate = self.out_rate.load(Ordering::Relaxed).max(1) as u64;
        self.base_ms.load(Ordering::Acquire) + self.frames_played.load(Ordering::Acquire) * 1000 / rate
    }
}

/// Runs inside the output callback. Must not block or allocate.
struct Renderer {
    consumer: Consumer<f32>,
    shared: Arc<Shared>,
    seen_generation: u64,
    awaiting_first: bool,
    scratch: Vec<f32>,
    channels: usize,
}

impl Renderer {
    fn render(&mut self, out: &mut [f32]) {
        let s = &self.shared;
        let generation = s.generation.load(Ordering::Acquire);
        if generation != self.seen_generation {
            let queued = self.consumer.slots();
            if let Ok(chunk) = self.consumer.read_chunk(queued) {
                chunk.commit_all();
            }
            s.frames_played.store(0, Ordering::Release);
            self.seen_generation = generation;
            self.awaiting_first = true;
            s.ack_generation.store(generation, Ordering::Release);
        }

        out.fill(0.0);
        if !s.playing.load(Ordering::Acquire) {
            return;
        }
        let volume = f32::from_bits(s.volume_bits.load(Ordering::Relaxed));
        let channels = self.channels.max(1);
        let frames_total = out.len() / channels;
        let mut frame = 0;
        while frame < frames_total {
            let frames = (frames_total - frame).min(self.scratch.len() / RING_CHANNELS);
            let want = frames * RING_CHANNELS;
            // Returns (filled, unfilled).
            let (filled, _) = self.consumer.pop_partial_slice(&mut self.scratch[..want]);
            let got = filled.len();
            let got_frames = got / RING_CHANNELS;
            for i in 0..got_frames {
                let l = self.scratch[i * 2] * volume;
                let r = self.scratch[i * 2 + 1] * volume;
                let o = (frame + i) * channels;
                if channels == 1 {
                    out[o] = 0.5 * (l + r);
                } else {
                    out[o] = l;
                    out[o + 1] = r;
                }
            }
            if got_frames > 0 {
                self.awaiting_first = false;
                s.frames_played.fetch_add(got_frames as u64, Ordering::AcqRel);
            }
            if got_frames < frames {
                if s.decoder_done.load(Ordering::Acquire) && self.consumer.slots() == 0 {
                    if !self.awaiting_first {
                        s.ended.store(true, Ordering::Release);
                        s.playing.store(false, Ordering::Release);
                    }
                } else if !self.awaiting_first {
                    s.underruns.fetch_add(1, Ordering::Relaxed);
                }
                return;
            }
            frame += frames;
        }
    }
}

enum Cmd {
    Load {
        decoder: Box<Decoder>,
        start_ms: u64,
        autoplay: bool,
    },
    Seek(u64),
    Stop,
    Shutdown,
}

pub struct Player {
    shared: Arc<Shared>,
    cmd: Sender<Cmd>,
    decoder_thread: Option<JoinHandle<()>>,
    output_stop: Option<Sender<()>>,
    output_thread: Option<JoinHandle<()>>,
}

impl Player {
    pub fn start(config: OutputConfig) -> Result<Player, String> {
        let shared = Arc::new(Shared::new());
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, usize, String, Producer<f32>), String>>();

        let out_shared = shared.clone();
        let output_thread = std::thread::Builder::new()
            .name("cd-audio-output".into())
            .spawn(move || match config {
                OutputConfig::Device => device::run(out_shared, ready_tx, stop_rx),
                OutputConfig::Null {
                    sample_rate,
                    channels,
                    period_frames,
                } => run_null(
                    out_shared,
                    sample_rate,
                    channels as usize,
                    period_frames,
                    ready_tx,
                    stop_rx,
                ),
            })
            .map_err(|e| e.to_string())?;

        let (rate, _channels, name, producer) = ready_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "audio output did not start".to_string())??;
        shared.out_rate.store(rate, Ordering::Release);
        shared.meta.lock().unwrap().output = name;

        let (cmd_tx, cmd_rx) = mpsc::channel();
        let dec_shared = shared.clone();
        let decoder_thread = std::thread::Builder::new()
            .name("cd-audio-decoder".into())
            .spawn(move || decoder_loop(dec_shared, producer, cmd_rx, rate))
            .map_err(|e| e.to_string())?;

        Ok(Player {
            shared,
            cmd: cmd_tx,
            decoder_thread: Some(decoder_thread),
            output_stop: Some(stop_tx),
            output_thread: Some(output_thread),
        })
    }

    /// Open `path` and queue it. Errors (missing, unsupported, corrupt) are
    /// returned here, before anything changes.
    pub fn load(&self, path: &Path, start_ms: u64, autoplay: bool) -> Result<AudioInfo, AudioError> {
        let decoder = Decoder::open(path)?;
        let info = decoder.info.clone();
        {
            let mut m = self.shared.meta.lock().unwrap();
            m.path = Some(path.to_path_buf());
            m.duration_ms = info.duration_ms;
            m.error = None;
            m.loaded = true;
        }
        self.shared.ended.store(false, Ordering::Release);
        // The decoder thread starts playback once the old audio is flushed.
        self.shared.playing.store(false, Ordering::Release);
        let _ = self.cmd.send(Cmd::Load {
            decoder: Box::new(decoder),
            start_ms,
            autoplay,
        });
        Ok(info)
    }

    pub fn play(&self) {
        if !self.shared.meta.lock().unwrap().loaded {
            return;
        }
        if self.shared.ended.swap(false, Ordering::AcqRel) {
            let _ = self.cmd.send(Cmd::Seek(0));
        }
        self.shared.playing.store(true, Ordering::Release);
    }

    pub fn pause(&self) {
        self.shared.playing.store(false, Ordering::Release);
    }

    pub fn toggle(&self) {
        if self.shared.playing.load(Ordering::Acquire) {
            self.pause()
        } else {
            self.play()
        }
    }

    pub fn seek(&self, ms: u64) {
        self.shared.ended.store(false, Ordering::Release);
        let _ = self.cmd.send(Cmd::Seek(ms));
    }

    pub fn set_volume(&self, volume: f32) {
        let v = volume.clamp(0.0, 1.0);
        self.shared.volume_bits.store(v.to_bits(), Ordering::Relaxed);
    }

    pub fn stop(&self) {
        self.shared.playing.store(false, Ordering::Release);
        let _ = self.cmd.send(Cmd::Stop);
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Acquire)
    }

    pub fn status(&self) -> PlayerStatus {
        let s = &self.shared;
        let m = s.meta.lock().unwrap();
        let state = if m.error.is_some() {
            PlayState::Error
        } else if !m.loaded {
            PlayState::Idle
        } else if s.ended.load(Ordering::Acquire) {
            PlayState::Ended
        } else if s.playing.load(Ordering::Acquire) {
            PlayState::Playing
        } else {
            PlayState::Paused
        };
        let mut position_ms = s.position_ms();
        if let Some(d) = m.duration_ms {
            position_ms = position_ms.min(d);
        }
        PlayerStatus {
            state,
            path: m.path.clone(),
            position_ms,
            duration_ms: m.duration_ms,
            volume: f32::from_bits(s.volume_bits.load(Ordering::Relaxed)),
            error: m.error.clone(),
            underruns: s.underruns.load(Ordering::Relaxed),
            output: m.output.clone(),
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.cmd.send(Cmd::Shutdown);
        if let Some(t) = self.decoder_thread.take() {
            let _ = t.join();
        }
        if let Some(stop) = self.output_stop.take() {
            let _ = stop.send(());
        }
        if let Some(t) = self.output_thread.take() {
            let _ = t.join();
        }
    }
}

fn ring_for(rate: u32) -> (Producer<f32>, Consumer<f32>) {
    RingBuffer::new(rate as usize * RING_CHANNELS * RING_SECONDS)
}

fn renderer(shared: Arc<Shared>, consumer: Consumer<f32>, channels: usize) -> Renderer {
    Renderer {
        consumer,
        shared,
        seen_generation: 0,
        awaiting_first: true,
        // Enough for any sane callback size; larger requests are split.
        scratch: vec![0.0; 16_384 * RING_CHANNELS],
        channels,
    }
}

type Ready = Sender<Result<(u32, usize, String, Producer<f32>), String>>;

fn run_null(
    shared: Arc<Shared>,
    rate: u32,
    channels: usize,
    period: usize,
    ready: Ready,
    stop: Receiver<()>,
) {
    let (producer, consumer) = ring_for(rate);
    let mut r = renderer(shared, consumer, channels);
    let _ = ready.send(Ok((rate, channels, format!("null output ({rate} Hz)"), producer)));
    let mut buf = vec![0.0f32; period * channels];
    let period_dur = Duration::from_secs_f64(period as f64 / rate as f64);
    let mut next = Instant::now();
    loop {
        match stop.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => return,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        r.render(&mut buf);
        next += period_dur;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
}

mod device {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SampleFormat, SizedSample};

    pub(super) fn run(shared: Arc<Shared>, ready: Ready, stop: Receiver<()>) {
        match build(shared) {
            Ok((stream, rate, channels, name, producer)) => {
                if let Err(e) = stream.play() {
                    let _ = ready.send(Err(format!("could not start audio output: {e}")));
                    return;
                }
                let _ = ready.send(Ok((rate, channels, name, producer)));
                // The stream lives on this thread until the player stops.
                let _ = stop.recv();
                drop(stream);
            }
            Err(e) => {
                let _ = ready.send(Err(e));
            }
        }
    }

    type Built = (cpal::Stream, u32, usize, String, Producer<f32>);

    fn build(shared: Arc<Shared>) -> Result<Built, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("No audio output device found. Connect speakers or headphones.")?;
        let name = device
            .id()
            .map(|d| d.to_string())
            .unwrap_or_else(|_| "default output".into());
        let supported = device
            .default_output_config()
            .map_err(|e| format!("audio output is not available: {e}"))?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let rate = config.sample_rate;
        let channels = config.channels as usize;
        let (producer, consumer) = ring_for(rate);
        let r = renderer(shared.clone(), consumer, channels);
        let stream = match format {
            SampleFormat::F32 => stream::<f32>(&device, config, r, shared),
            SampleFormat::I16 => stream::<i16>(&device, config, r, shared),
            SampleFormat::I32 => stream::<i32>(&device, config, r, shared),
            SampleFormat::U16 => stream::<u16>(&device, config, r, shared),
            other => return Err(format!("unsupported output sample format {other}")),
        }?;
        Ok((stream, rate, channels, format!("{name} ({rate} Hz)"), producer))
    }

    fn stream<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        mut r: Renderer,
        shared: Arc<Shared>,
    ) -> Result<cpal::Stream, String>
    where
        T: SizedSample + FromSample<f32>,
    {
        let mut f32_buf: Vec<f32> = vec![0.0; 16_384 * 8];
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                    if data.len() > f32_buf.len() {
                        // Never allocate in the callback; play silence for an
                        // oversized request instead.
                        data.iter_mut().for_each(|s| *s = T::from_sample(0.0));
                        return;
                    }
                    let buf = &mut f32_buf[..data.len()];
                    r.render(buf);
                    for (o, i) in data.iter_mut().zip(buf.iter()) {
                        *o = T::from_sample(*i);
                    }
                },
                move |err| {
                    tracing::warn!("audio output error: {err}");
                    shared.meta.lock().unwrap().error = Some(format!("Audio output error: {err}"));
                    shared.playing.store(false, Ordering::Release);
                },
                None,
            )
            .map_err(|e| format!("could not open audio output: {e}"))
    }
}

/// Converts the file's channels to stereo.
fn to_stereo(input: &[f32], channels: usize, out: &mut Vec<f32>) {
    out.clear();
    match channels {
        0 => {}
        1 => {
            for s in input {
                out.push(*s);
                out.push(*s);
            }
        }
        2 => out.extend_from_slice(input),
        n => {
            for f in input.chunks_exact(n) {
                out.push(f[0]);
                out.push(f[1]);
            }
        }
    }
}

struct Track {
    decoder: Box<Decoder>,
    resampler: StereoResampler,
    /// Frames to drop after an accurate seek that landed early.
    skip_frames: u64,
    /// The decoder has no more packets.
    eof: bool,
}

fn decoder_loop(shared: Arc<Shared>, mut producer: Producer<f32>, rx: Receiver<Cmd>, out_rate: u32) {
    let mut track: Option<Track> = None;
    let mut decoded = Vec::new();
    let mut stereo = Vec::new();
    // Resampled audio waiting for room in the ring.
    let mut pending: Vec<f32> = Vec::new();
    let mut pending_pos = 0usize;

    // Ask the output to discard everything queued, and wait until it has.
    let flush = |shared: &Shared, base_ms: u64| {
        shared.decoder_done.store(false, Ordering::Release);
        shared.base_ms.store(base_ms, Ordering::Release);
        shared.frames_played.store(0, Ordering::Release);
        let g = shared.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let deadline = Instant::now() + Duration::from_millis(500);
        while shared.ack_generation.load(Ordering::Acquire) != g && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
    };

    loop {
        let has_pending = pending_pos < pending.len();
        let can_decode = track.as_ref().map(|t| !t.eof).unwrap_or(false);
        let wait = if (has_pending || can_decode) && producer.slots() > 4096 {
            Duration::ZERO
        } else {
            Duration::from_millis(10)
        };
        match rx.recv_timeout(wait) {
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) | Ok(Cmd::Shutdown) => return,
            Ok(Cmd::Stop) => {
                track = None;
                pending.clear();
                pending_pos = 0;
                flush(&shared, 0);
                shared.decoder_done.store(true, Ordering::Release);
                let mut m = shared.meta.lock().unwrap();
                m.loaded = false;
                m.path = None;
                continue;
            }
            Ok(Cmd::Load {
                mut decoder,
                start_ms,
                autoplay,
            }) => {
                pending.clear();
                pending_pos = 0;
                let mut skip = 0;
                let mut base = 0;
                if start_ms > 0 {
                    match decoder.seek_ms(start_ms) {
                        Ok(actual) => {
                            base = start_ms;
                            skip = start_ms.saturating_sub(actual) * decoder.info.sample_rate as u64 / 1000;
                        }
                        Err(e) => tracing::warn!("initial seek failed: {e}"),
                    }
                }
                let resampler = StereoResampler::new(decoder.info.sample_rate, out_rate);
                track = Some(Track {
                    decoder,
                    resampler,
                    skip_frames: skip,
                    eof: false,
                });
                flush(&shared, base);
                shared.playing.store(autoplay, Ordering::Release);
                continue;
            }
            Ok(Cmd::Seek(ms)) => {
                if let Some(t) = track.as_mut() {
                    pending.clear();
                    pending_pos = 0;
                    match t.decoder.seek_ms(ms) {
                        Ok(actual) => {
                            t.skip_frames =
                                ms.saturating_sub(actual) * t.decoder.info.sample_rate as u64 / 1000;
                            t.resampler.reset();
                            t.eof = false;
                            flush(&shared, ms);
                        }
                        Err(e) => {
                            shared.meta.lock().unwrap().error = Some(e.user_message());
                            shared.playing.store(false, Ordering::Release);
                        }
                    }
                }
                continue;
            }
        }

        // Push already resampled audio first.
        if pending_pos < pending.len() {
            let (written, _) = producer.push_partial_slice(&pending[pending_pos..]);
            pending_pos += written.len();
            if pending_pos < pending.len() {
                continue;
            }
            pending.clear();
            pending_pos = 0;
        }

        let Some(t) = track.as_mut() else { continue };
        if t.eof {
            // Everything is in the ring; the output reports the end once it
            // has played it.
            shared.decoder_done.store(true, Ordering::Release);
            continue;
        }
        match t.decoder.next_chunk(&mut decoded) {
            Ok(true) => {
                let channels = t.decoder.info.channels.max(1) as usize;
                to_stereo(&decoded, channels, &mut stereo);
                if t.skip_frames > 0 {
                    let frames = (stereo.len() / RING_CHANNELS) as u64;
                    let drop = t.skip_frames.min(frames);
                    stereo.drain(..(drop as usize * RING_CHANNELS));
                    t.skip_frames -= drop;
                }
                t.resampler.process(&stereo, &mut pending);
            }
            Ok(false) => {
                t.resampler.finish(&mut pending);
                t.eof = true;
            }
            Err(e) => {
                tracing::warn!("playback decode error: {e}");
                t.eof = true;
                shared.decoder_done.store(true, Ordering::Release);
                shared.meta.lock().unwrap().error = Some(e.user_message());
                shared.playing.store(false, Ordering::Release);
            }
        }
    }
}
