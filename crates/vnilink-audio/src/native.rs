//! Explicit-device native PCM I/O. Enumeration does not open a device. No
//! fallback to the default microphone after unplug or failed authorization.
use crate::{AudioEncoder, AudioPacket, AudioResampler, Error, Format, FRAME_SAMPLES, SAMPLE_RATE};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceDirection {
    Input,
    Output,
    Loopback,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub direction: DeviceDirection,
}

fn host() -> Result<cpal::Host, Error> {
    #[cfg(target_os = "linux")]
    {
        cpal::host_from_id(cpal::HostId::PulseAudio)
            .map_err(|_| Error::DeviceStage("connect PulseAudio"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(cpal::default_host())
    }
}
/// Resolve the OS-selected endpoint without activating it. Linux system audio
/// follows the monitor of the default sink; Windows uses that render endpoint.
pub fn default_device_id(direction: DeviceDirection) -> Result<Option<String>, Error> {
    let host = host()?;
    let selected = match direction {
        DeviceDirection::Input => host.default_input_device(),
        DeviceDirection::Output => host.default_output_device(),
        DeviceDirection::Loopback => {
            #[cfg(target_os = "linux")]
            {
                let cpal::platform::HostInner::PulseAudio(pulse) = host.as_inner() else {
                    return Ok(None);
                };
                pulse.default_loopback_device().map(Into::into)
            }
            #[cfg(target_os = "windows")]
            {
                host.default_output_device()
            }
            #[cfg(not(any(target_os = "linux", target_os = "windows")))]
            {
                None
            }
        }
    };
    selected
        .map(|device| {
            device
                .id()
                .map(|id| id.to_string())
                .map_err(|_| Error::Device)
        })
        .transpose()
}

pub fn audio_devices() -> Result<Vec<AudioDevice>, Error> {
    let host = host()?;
    let mut result = Vec::new();
    for device in host.devices().map_err(|_| Error::Device)?.take(128) {
        let Ok(id) = device.id() else { continue };
        let Ok(description) = device.description() else {
            continue;
        };
        let id = id.to_string();
        if id.is_empty() || id.len() > 512 {
            continue;
        }
        let name: String = description.name().chars().take(256).collect();
        if device.supports_input() && description.direction() != cpal::DeviceDirection::Output {
            result.push(AudioDevice {
                id: id.clone(),
                name: name.clone(),
                direction: if device.is_loopback_source() {
                    DeviceDirection::Loopback
                } else {
                    DeviceDirection::Input
                },
            });
        }
        if device.supports_output() {
            result.push(AudioDevice {
                id: id.clone(),
                name: name.clone(),
                direction: DeviceDirection::Output,
            });
            #[cfg(target_os = "windows")]
            result.push(AudioDevice {
                id,
                name,
                direction: DeviceDirection::Loopback,
            });
        }
    }
    Ok(result)
}
fn device(id: &str) -> Result<cpal::Device, Error> {
    if id.is_empty() || id.len() > 512 {
        return Err(Error::Device);
    }
    let parsed = id
        .parse()
        .map_err(|_| Error::DeviceStage("parse selected ID"))?;
    host()?
        .device_by_id(&parsed)
        .ok_or(Error::DeviceStage("find selected ID"))
}
fn format(channels: u16) -> Result<Format, Error> {
    match channels {
        1 => Ok(Format::Mono),
        2 => Ok(Format::Stereo),
        _ => Err(Error::InvalidFormat),
    }
}
struct Buffer {
    failed: AtomicBool,
    failure_reason: AtomicU64,
    underruns: AtomicU64,
}
impl Buffer {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            failed: AtomicBool::new(false),
            failure_reason: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
        })
    }
}
fn capture_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    output: Format,
    mut producer: rtrb::Producer<f32>,
    buffer: Arc<Buffer>,
) -> Result<cpal::Stream, Error>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let native_channels = config.channels as usize;
    let mut config = config;
    config.buffer_size = cpal::BufferSize::Fixed(config.sample_rate / 100);
    let failure = buffer.clone();
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                if buffer.failed.load(Ordering::Acquire) {
                    return;
                }
                let needed = data.len() / native_channels * output.channels();
                if !data.len().is_multiple_of(native_channels) || needed > producer.slots() {
                    buffer.failure_reason.store(3, Ordering::Relaxed);
                    buffer.failed.store(true, Ordering::Release);
                    return;
                }
                for frame in data.chunks_exact(native_channels) {
                    let left = frame[0].to_sample::<f32>();
                    let right = if native_channels == 2 {
                        frame[1].to_sample::<f32>()
                    } else {
                        left
                    };
                    if !left.is_finite() || !right.is_finite() {
                        buffer.failed.store(true, Ordering::Release);
                        return;
                    }
                    if output == Format::Mono {
                        let _ = producer.push(((left + right) * 0.5).clamp(-1.0, 1.0));
                    } else {
                        let _ = producer.push(left.clamp(-1.0, 1.0));
                        let _ = producer.push(right.clamp(-1.0, 1.0));
                    }
                }
            },
            move |_| {
                failure.failure_reason.store(1, Ordering::Relaxed);
                failure.failed.store(true, Ordering::Release);
            },
            Some(Duration::from_secs(5)),
        )
        .map_err(|_| Error::DeviceStage("build native stream"))
}

pub struct NativeCapture {
    // Drop stream before callback queue and codec; CPAL stops native callbacks.
    _stream: cpal::Stream,
    buffer: Arc<Buffer>,
    consumer: rtrb::Consumer<f32>,
    resampler: AudioResampler,
    remainder: VecDeque<f32>,
    encoder: AudioEncoder,
    format: Format,
}
impl NativeCapture {
    /// Caller must check local source approval and OS microphone authorization
    /// before calling. Loopback is WASAPI render capture on Windows only.
    pub fn open(
        id: &str,
        direction: DeviceDirection,
        output: Format,
        voice: bool,
    ) -> Result<Self, Error> {
        if direction == DeviceDirection::Output {
            return Err(Error::Device);
        }
        #[cfg(target_os = "macos")]
        if direction == DeviceDirection::Loopback {
            return Err(Error::Device);
        }
        let device = device(id)?;
        #[cfg(target_os = "linux")]
        if (direction == DeviceDirection::Loopback) != device.is_loopback_source() {
            return Err(Error::Device);
        }
        let config = if cfg!(target_os = "windows") && direction == DeviceDirection::Loopback {
            device.default_output_config()
        } else {
            device.default_input_config()
        }
        .map_err(|_| Error::DeviceStage("input configuration"))?;
        format(config.channels())?;
        let rate = config.sample_rate();
        let resampler = AudioResampler::new(rate, SAMPLE_RATE, output, (rate / 100) as usize)?;
        let encoder = AudioEncoder::new(output, voice)?;
        // At most 100 ms of PCM at the actual native rate.
        let buffer = Buffer::new();
        let (producer, consumer) = rtrb::RingBuffer::new((rate / 10) as usize * output.channels());
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                capture_stream::<f32>(&device, config.config(), output, producer, buffer.clone())?
            }
            cpal::SampleFormat::I16 => {
                capture_stream::<i16>(&device, config.config(), output, producer, buffer.clone())?
            }
            cpal::SampleFormat::U16 => {
                capture_stream::<u16>(&device, config.config(), output, producer, buffer.clone())?
            }
            _ => return Err(Error::InvalidFormat),
        };
        stream
            .play()
            .map_err(|_| Error::DeviceStage("start capture"))?;
        Ok(Self {
            _stream: stream,
            buffer,
            consumer,
            resampler,
            remainder: VecDeque::with_capacity(4 * FRAME_SAMPLES * output.channels()),
            encoder,
            format: output,
        })
    }
    pub fn next_packet(&mut self) -> Result<Option<AudioPacket>, Error> {
        if self.buffer.failed.load(Ordering::Acquire) {
            return Err(Error::DeviceStage(
                match self.buffer.failure_reason.load(Ordering::Relaxed) {
                    2 => "capture callback contention",
                    3 => "capture PCM overrun",
                    _ => "native stream callback",
                },
            ));
        }
        let count = FRAME_SAMPLES * self.format.channels();
        while self.remainder.len() < count {
            let needed = self.resampler.input_samples();
            if self.consumer.slots() < needed {
                return Ok(None);
            }
            let input: Vec<_> = (0..needed).map(|_| self.consumer.pop().unwrap()).collect();
            self.remainder.extend(self.resampler.process(&input)?);
        }
        let pcm: Vec<_> = self.remainder.drain(..count).collect();
        Ok(Some(self.encoder.encode(&pcm)?))
    }
}

fn output_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut consumer: rtrb::Consumer<f32>,
    buffer: Arc<Buffer>,
) -> Result<cpal::Stream, Error>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let failure = buffer.clone();
    let mut config = config;
    config.buffer_size = cpal::BufferSize::Fixed(config.sample_rate / 100);
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                data.fill(T::EQUILIBRIUM);
                let mut missing = false;
                for sample in data {
                    if let Ok(value) = consumer.pop() {
                        *sample = T::from_sample(value);
                    } else {
                        missing = true;
                    }
                }
                if missing {
                    buffer.underruns.fetch_add(1, Ordering::Relaxed);
                }
            },
            move |_| {
                failure.failure_reason.store(1, Ordering::Relaxed);
                failure.failed.store(true, Ordering::Release);
            },
            Some(Duration::from_secs(5)),
        )
        .map_err(|_| Error::DeviceStage("build native stream"))
}
pub struct NativeOutput {
    _stream: cpal::Stream,
    buffer: Arc<Buffer>,
    resampler: AudioResampler,
    producer: rtrb::Producer<f32>,
    capacity: usize,
    input_format: Format,
    output_format: Format,
    target_samples: usize,
    drift_ppm: f64,
    started: bool,
}
impl NativeOutput {
    pub fn open(id: &str, input: Format) -> Result<Self, Error> {
        let device = device(id)?;
        let config = device.default_output_config().map_err(|_| Error::Device)?;
        let output = format(config.channels())?;
        let rate = config.sample_rate();
        let resampler = AudioResampler::new(SAMPLE_RATE, rate, output, FRAME_SAMPLES)?;
        let capacity = (rate / 5) as usize * output.channels();
        let (producer, consumer) = rtrb::RingBuffer::new(capacity);
        let buffer = Buffer::new();
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                output_stream::<f32>(&device, config.config(), consumer, buffer.clone())?
            }
            cpal::SampleFormat::I16 => {
                output_stream::<i16>(&device, config.config(), consumer, buffer.clone())?
            }
            cpal::SampleFormat::U16 => {
                output_stream::<u16>(&device, config.config(), consumer, buffer.clone())?
            }
            _ => return Err(Error::InvalidFormat),
        };
        Ok(Self {
            _stream: stream,
            buffer,
            resampler,
            producer,
            capacity,
            input_format: input,
            output_format: output,
            target_samples: (rate as usize * 3 / 50) * output.channels(),
            drift_ppm: 0.0,
            started: false,
        })
    }
    /// One decoded 20 ms block. Muted blocks are zero samples supplied by the
    /// playback engine; capture and decoding are unaffected by receiver mute.
    pub fn push(&mut self, samples: &[f32]) -> Result<(), Error> {
        if self.buffer.failed.load(Ordering::Acquire) {
            return Err(Error::DeviceFailed);
        }
        if samples.len() != FRAME_SAMPLES * self.input_format.channels()
            || samples.iter().any(|v| !v.is_finite())
        {
            return Err(Error::InvalidFormat);
        }
        let mut converted = Vec::with_capacity(FRAME_SAMPLES * self.output_format.channels());
        for frame in samples.chunks_exact(self.input_format.channels()) {
            let left = frame[0];
            let right = if frame.len() == 2 { frame[1] } else { left };
            if self.output_format == Format::Mono {
                converted.push((left + right) * 0.5);
            } else {
                converted.push(left);
                converted.push(right);
            }
        }
        let queued = self.capacity - self.producer.slots();
        // More queued PCM requires fewer output samples, not a sender clock
        // change. A gentle bounded servo avoids accumulating device-clock drift.
        let error = (self.target_samples as f64 - queued as f64) / self.target_samples as f64;
        if self.started {
            self.drift_ppm = crate::resample::drift_step(self.drift_ppm, error);
        }
        self.resampler.set_drift_ppm(self.drift_ppm)?;
        let out = self.resampler.process(&converted)?;
        if out.len() > self.producer.slots() {
            return Err(Error::DeviceStage("playback PCM overrun"));
        }
        for sample in out {
            self.producer
                .push(sample)
                .map_err(|_| Error::DeviceFailed)?;
        }
        let ready = self.capacity - self.producer.slots() >= self.target_samples;
        if !self.started && ready {
            self._stream
                .play()
                .map_err(|_| Error::DeviceStage("start capture"))?;
            self.started = true;
        }
        Ok(())
    }
    pub fn queued_samples(&self) -> Result<usize, Error> {
        Ok(self.capacity - self.producer.slots())
    }
    pub fn underruns(&self) -> u64 {
        self.buffer.underruns.load(Ordering::Relaxed)
    }
}
