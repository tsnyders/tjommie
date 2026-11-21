//! Capturing the microphone for dictation, and ending the recording when the window loses focus.
//!
//! `cpal` (WASAPI) delivers whatever the device gives: any sample format, any channel count, any rate. Everything
//! after that is plain arithmetic, tested on fixtures: mix to mono, resample to 16 kHz, quantize to PCM16, cut into
//! chunks. A `Pipeline` does that, so no test needs a microphone. The cpal stream lives on its own thread (a stream
//! isn't `Send`), which is also where the device is opened and closed.

use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use protocol::speech::{SPEECH_LIMITS, SpeechStart};

use super::runtime::{PcmChunk, SpeechRuntime};

/// Average of each frame's channels.
pub fn mono_from(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Signed 16-bit samples, clamped to [-1, 1].
pub fn pcm16_from(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|sample| (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16)
        .collect()
}

/// Linear-interpolation resampling that carries its place from one buffer to the next, so a stream cut into device
/// callbacks comes out as one continuous signal. Speech at 44.1 and 48 kHz has little above 8 kHz to alias, so there
/// is no separate low-pass.
pub struct Resampler {
    /// Input samples per output sample.
    step: f64,
    /// The next output's place, in input samples, counted from `previous`.
    position: f64,
    previous: Option<f32>,
}

impl Resampler {
    pub fn new(input_rate: u32, output_rate: u32) -> Resampler {
        Resampler {
            step: f64::from(input_rate) / f64::from(output_rate.max(1)),
            // The buffer read is `previous` then the input, so the first input sample is at 1.
            position: 1.0,
            previous: None,
        }
    }

    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let Some(first) = input.first().copied() else {
            return Vec::new();
        };
        if (self.step - 1.0).abs() < f64::EPSILON {
            return input.to_vec();
        }
        let previous = *self.previous.get_or_insert(first);
        // The buffer being read is `previous` followed by `input`.
        let at = |index: usize| {
            if index == 0 {
                previous
            } else {
                input.get(index - 1).copied().unwrap_or(previous)
            }
        };
        let mut out = Vec::with_capacity((input.len() as f64 / self.step) as usize + 1);
        loop {
            let index = self.position.floor() as usize;
            if index + 1 > input.len() {
                break;
            }
            let fraction = (self.position - index as f64) as f32;
            let (a, b) = (at(index), at(index + 1));
            out.push(a + (b - a) * fraction);
            self.position += self.step;
        }
        self.previous = input.last().copied();
        self.position -= input.len() as f64;
        out
    }
}

/// What a device callback turns into: mono, 16 kHz, PCM16, in chunks of a fixed size.
pub struct Pipeline {
    channels: usize,
    resampler: Resampler,
    chunk_bytes: usize,
    pending: Vec<u8>,
}

/// 100 ms of 16 kHz PCM16.
pub const CHUNK_BYTES: usize = 3200;

impl Pipeline {
    pub fn new(input_rate: u32, channels: usize) -> Pipeline {
        Pipeline {
            channels,
            resampler: Resampler::new(input_rate, SPEECH_LIMITS.sample_rate),
            chunk_bytes: CHUNK_BYTES,
            pending: Vec::new(),
        }
    }

    /// Feeds interleaved samples; returns the chunks that are complete.
    pub fn push(&mut self, interleaved: &[f32]) -> Vec<Vec<u8>> {
        let mono = mono_from(interleaved, self.channels);
        let resampled = self.resampler.process(&mono);
        self.pending.extend(
            pcm16_from(&resampled)
                .into_iter()
                .flat_map(i16::to_le_bytes),
        );
        let mut chunks = Vec::new();
        while self.pending.len() >= self.chunk_bytes {
            let rest = self.pending.split_off(self.chunk_bytes);
            chunks.push(std::mem::replace(&mut self.pending, rest));
        }
        chunks
    }

    /// What is left when the recording ends (possibly nothing).
    pub fn finish(&mut self) -> Option<Vec<u8>> {
        (!self.pending.is_empty()).then(|| std::mem::take(&mut self.pending))
    }
}

/// Where a capture delivers its chunks.
pub type Sink = Arc<dyn Fn(Vec<u8>) + Send + Sync>;

/// A running capture. Stopping it (or dropping it) closes the device.
pub trait CaptureHandle: Send {
    fn stop(self: Box<Self>);
}

/// Where microphone audio comes from. `sink` is called with each chunk of PCM16 (16 kHz, mono, little-endian), in
/// order, until the handle is stopped; the last call is the remainder.
pub trait CaptureSource: Send + Sync {
    fn start(&self, sink: Sink) -> Result<Box<dyn CaptureHandle>, String>;
}

/// The real microphone: the default input device, through WASAPI.
pub struct CpalSource;

struct CpalHandle {
    stop: Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl CaptureHandle for CpalHandle {
    fn stop(mut self: Box<Self>) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for CpalHandle {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

const NO_MICROPHONE: &str = "No microphone could be opened. Check that one is connected and that Windows lets desktop apps use it.";

/// Builds the input stream for one sample format.
fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    pipeline: Arc<Mutex<Pipeline>>,
    sink: Sink,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + Send + 'static,
    f32: cpal::FromSample<T>,
{
    device
        .build_input_stream(
            (*config).into(),
            move |data: &[T], _: &_| {
                let floats: Vec<f32> = data
                    .iter()
                    .map(|sample| sample.to_sample::<f32>())
                    .collect();
                let chunks = pipeline
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(&floats);
                for chunk in chunks {
                    sink(chunk);
                }
            },
            |error| log::warn!("The microphone reported a problem: {error}"),
            None,
        )
        .map_err(|_| NO_MICROPHONE.to_string())
}

impl CaptureSource for CpalSource {
    fn start(&self, sink: Sink) -> Result<Box<dyn CaptureHandle>, String> {
        let (ready_tx, ready_rx) = channel::<Result<(), String>>();
        let (stop_tx, stop_rx) = channel::<()>();
        let thread = std::thread::Builder::new()
            .name("microphone".into())
            .spawn(move || {
                let opened = (|| {
                    let device = cpal::default_host()
                        .default_input_device()
                        .ok_or_else(|| NO_MICROPHONE.to_string())?;
                    let config = device
                        .default_input_config()
                        .map_err(|_| NO_MICROPHONE.to_string())?;
                    let pipeline = Arc::new(Mutex::new(Pipeline::new(
                        config.sample_rate(),
                        usize::from(config.channels()),
                    )));
                    let stream = match config.sample_format() {
                        cpal::SampleFormat::F32 => build_stream::<f32>(
                            &device,
                            &config,
                            Arc::clone(&pipeline),
                            Arc::clone(&sink),
                        ),
                        cpal::SampleFormat::I16 => build_stream::<i16>(
                            &device,
                            &config,
                            Arc::clone(&pipeline),
                            Arc::clone(&sink),
                        ),
                        cpal::SampleFormat::I32 => build_stream::<i32>(
                            &device,
                            &config,
                            Arc::clone(&pipeline),
                            Arc::clone(&sink),
                        ),
                        cpal::SampleFormat::U16 => build_stream::<u16>(
                            &device,
                            &config,
                            Arc::clone(&pipeline),
                            Arc::clone(&sink),
                        ),
                        _ => Err("The microphone's sample format isn't supported.".to_string()),
                    }?;
                    stream.play().map_err(|_| NO_MICROPHONE.to_string())?;
                    Ok::<_, String>((stream, pipeline))
                })();
                match opened {
                    Ok((stream, pipeline)) => {
                        let _ = ready_tx.send(Ok(()));
                        let _ = stop_rx.recv();
                        drop(stream);
                        let rest = pipeline
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .finish();
                        if let Some(rest) = rest {
                            sink(rest);
                        }
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                    }
                }
            })
            .map_err(|_| NO_MICROPHONE.to_string())?;
        ready_rx.recv().map_err(|_| NO_MICROPHONE.to_string())??;
        Ok(Box::new(CpalHandle {
            stop: stop_tx,
            thread: Some(thread),
        }))
    }
}

/// What the pump is told: audio from the device, or that the person is done (`Finish`) or has thrown it away (`Abort`).
enum Heard {
    Chunk(Vec<u8>),
    Finish,
    Abort,
}

struct Recording {
    id: String,
    /// The pump owns the microphone. It ends, and closes it, when the person finishes or cancels **and** when the runtime
    /// ends the recording on its own (the 60 s limit, a refused chunk, a failed write, a cancel from elsewhere): the next
    /// chunk is refused and the loop stops. Nothing else holds the capture, so nothing can leave it running.
    pump: tokio::task::JoinHandle<()>,
    heard: tokio::sync::mpsc::UnboundedSender<Heard>,
}

/// A dictation: the runtime plus a capture source. `on_window_blur` is what the window's blur event calls
/// (`main.ts:640`): a recording with audio in it is cancelled, so it never keeps listening behind another window.
pub struct Dictation {
    runtime: SpeechRuntime,
    source: Arc<dyn CaptureSource>,
    recording: Mutex<Option<Recording>>,
}

/// Closes the microphone on the blocking pool: stopping joins the capture thread.
async fn close(capture: Box<dyn CaptureHandle>) {
    let _ = tokio::task::spawn_blocking(move || capture.stop()).await;
}

impl Dictation {
    pub fn new(runtime: SpeechRuntime, source: Arc<dyn CaptureSource>) -> Dictation {
        Dictation {
            runtime,
            source,
            recording: Mutex::new(None),
        }
    }

    /// Starts the runtime's session and the microphone.
    pub async fn begin(&self, session_id: &str) -> Result<(), String> {
        self.runtime
            .start(SpeechStart {
                session_id: session_id.into(),
                language: "en".into(),
            })
            .await?;
        let (heard, mut received) = tokio::sync::mpsc::unbounded_channel::<Heard>();
        let sender = heard.clone();
        let sink: Sink = Arc::new(move |pcm| {
            let _ = sender.send(Heard::Chunk(pcm));
        });
        // Opening the device waits on WASAPI, seconds when the audio service is cold: on the blocking pool, so the
        // runtime's workers (the daemon's calls among them) go on meanwhile (M4 re-review R4).
        let source = Arc::clone(&self.source);
        let opened = tokio::task::spawn_blocking(move || source.start(sink))
            .await
            .unwrap_or_else(|_| Err(NO_MICROPHONE.to_string()));
        let capture = match opened {
            Ok(capture) => capture,
            Err(error) => {
                let _ = self.runtime.cancel(session_id).await;
                return Err(error);
            }
        };
        let runtime = self.runtime.clone();
        let id = session_id.to_string();
        let pump = tokio::spawn(async move {
            let mut capture = Some(capture);
            let mut sequence = 0u32;
            let mut append = async |pcm: Vec<u8>| {
                let chunk = PcmChunk {
                    session_id: id.clone(),
                    sequence,
                    pcm,
                };
                sequence += 1;
                runtime.append_pcm(chunk).await.is_ok()
            };
            while let Some(message) = received.recv().await {
                match message {
                    Heard::Chunk(pcm) => {
                        if !append(pcm).await {
                            break;
                        }
                    }
                    Heard::Finish => {
                        // Stopping delivers what the device still had, ahead of anything not yet read here.
                        if let Some(capture) = capture.take() {
                            close(capture).await;
                        }
                        while let Ok(Heard::Chunk(pcm)) = received.try_recv() {
                            if !append(pcm).await {
                                break;
                            }
                        }
                        break;
                    }
                    Heard::Abort => break,
                }
            }
            if let Some(capture) = capture.take() {
                close(capture).await;
            }
        });
        *self
            .recording
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Recording {
            id: session_id.into(),
            pump,
            heard,
        });
        Ok(())
    }

    fn take(&self) -> Option<Recording> {
        self.recording
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// The user is done speaking: closes the microphone, lets what it captured reach the runtime, then transcribes.
    pub async fn finish(&self) -> Result<(), String> {
        let Some(recording) = self.take() else {
            return Err("This recording is no longer active.".into());
        };
        let _ = recording.heard.send(Heard::Finish);
        let _ = recording.pump.await;
        self.runtime.stop(&recording.id).await
    }

    /// Throws the recording away.
    pub async fn cancel(&self) -> Result<(), String> {
        let Some(recording) = self.take() else {
            return Ok(());
        };
        let _ = recording.heard.send(Heard::Abort);
        let _ = recording.pump.await;
        self.runtime.cancel(&recording.id).await
    }

    /// The window lost focus. A recording that has audio in it is cancelled; one that hasn't heard anything yet is
    /// left alone, as in `main.ts`. True if it cancelled.
    pub async fn on_window_blur(&self) -> bool {
        if !self.runtime.is_recording_audio() {
            return false;
        }
        if let Err(error) = self.cancel().await {
            log::warn!("Temporary speech cleanup failed: {error}");
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hertz: f32, seconds: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .map(|i| (2.0 * std::f32::consts::PI * hertz * i as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    #[test]
    fn mixes_to_mono_and_quantizes() {
        assert_eq!(mono_from(&[1.0, 0.0, 0.5, 0.5], 2), [0.5, 0.5]);
        assert_eq!(mono_from(&[0.25, 0.5], 1), [0.25, 0.5]);
        assert_eq!(
            pcm16_from(&[0.0, 1.0, -1.0, 2.0, -2.0, 0.5]),
            [0, 32767, -32767, 32767, -32767, 16384]
        );
    }

    #[test]
    fn resamples_48k_to_16k_keeping_the_tone() {
        let mut resampler = Resampler::new(48_000, 16_000);
        let out = resampler.process(&sine(48_000, 440.0, 1.0));
        assert!(
            (out.len() as i64 - 16_000).abs() <= 1,
            "one second in, one second out: {}",
            out.len()
        );
        // A 440 Hz tone sampled at 16 kHz is where a sine says it should be, to within interpolation error.
        for (i, sample) in out.iter().enumerate().skip(10).take(2_000) {
            let expected = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin() * 0.5;
            assert!(
                (sample - expected).abs() < 0.02,
                "sample {i}: {sample} vs {expected}"
            );
        }
        // A constant stays constant, and 44.1 kHz works too.
        let mut resampler = Resampler::new(44_100, 16_000);
        let out = resampler.process(&vec![0.25; 44_100]);
        assert!((out.len() as i64 - 16_000).abs() <= 1);
        assert!(out.iter().all(|sample| (sample - 0.25).abs() < 1e-6));
        // The same rate passes through.
        assert_eq!(
            Resampler::new(16_000, 16_000).process(&[0.1, 0.2]),
            [0.1, 0.2]
        );
        assert!(Resampler::new(48_000, 16_000).process(&[]).is_empty());
    }

    #[test]
    fn a_stream_cut_into_callbacks_comes_out_the_same_as_one_buffer() {
        let signal = sine(48_000, 300.0, 0.5);
        let whole = Resampler::new(48_000, 16_000).process(&signal);
        let mut pieces = Resampler::new(48_000, 16_000);
        let mut cut = Vec::new();
        // Device callbacks of an awkward size, not a multiple of the ratio.
        for piece in signal.chunks(441) {
            cut.extend(pieces.process(piece));
        }
        assert_eq!(whole.len(), cut.len());
        for (a, b) in whole.iter().zip(&cut) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn the_pipeline_makes_even_bounded_chunks_and_a_remainder() {
        let stereo: Vec<f32> = sine(48_000, 220.0, 1.0)
            .iter()
            .flat_map(|s| [*s, *s])
            .collect();
        let mut pipeline = Pipeline::new(48_000, 2);
        let mut bytes = 0;
        let mut chunks = 0;
        for piece in stereo.chunks(1024) {
            for chunk in pipeline.push(piece) {
                assert_eq!(chunk.len(), CHUNK_BYTES);
                assert!(chunk.len() <= SPEECH_LIMITS.chunk_bytes && chunk.len().is_multiple_of(2));
                bytes += chunk.len();
                chunks += 1;
            }
        }
        let rest = pipeline.finish().unwrap_or_default();
        assert!(rest.len() < CHUNK_BYTES && rest.len().is_multiple_of(2));
        // One second of 16 kHz PCM16 is 32,000 bytes (give or take the resampler's edge sample).
        assert!(
            (bytes + rest.len()).abs_diff(32_000) <= 2,
            "{}",
            bytes + rest.len()
        );
        assert_eq!(chunks, 10);
        assert!(pipeline.finish().is_none());
    }
}
