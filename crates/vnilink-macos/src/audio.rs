//! Audio-only ScreenCaptureKit streams. Local approval and existing permission
//! precede activation; this adapter never requests permission or changes outputs.
use screencapturekit::{
    cm::CMSampleBuffer,
    shareable_content::SCShareableContent,
    stream::{
        configuration::SCStreamConfiguration, content_filter::SCContentFilter,
        delegate_trait::StreamCallbacks, output_trait::SCStreamOutputTrait,
        output_type::SCStreamOutputType, SCStream,
    },
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{sync_channel, Receiver, SyncSender},
    Arc,
};
use vnilink::audio::{AudioDevice, AudioKind};
use vnilink_audio::{AudioEncoder, AudioPacket, Format, FRAME_SAMPLES};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}
fn permission() -> bool {
    // Probe only. No CGRequestScreenCaptureAccess call occurs on a remote path.
    unsafe { CGPreflightScreenCaptureAccess() }
}
fn selected_id(id: &str) -> Result<u32, vnilink::Error> {
    let value = id
        .strip_prefix("sck-system:")
        .ok_or(vnilink::Error::Malformed)?;
    let parsed: u32 = value.parse().map_err(|_| vnilink::Error::Malformed)?;
    if parsed == 0 || value != parsed.to_string() {
        return Err(vnilink::Error::Malformed);
    }
    Ok(parsed)
}
pub fn system_audio_devices() -> Result<Vec<AudioDevice>, vnilink::Error> {
    if !permission() {
        return Ok(Vec::new());
    }
    let content = SCShareableContent::get()
        .map_err(|_| vnilink::Error::Unsupported("system audio permission or display"))?;
    Ok(content
        .displays()
        .iter()
        .take(64)
        .map(|d| AudioDevice {
            id: format!("sck-system:{}", d.display_id()),
            name: format!("System sound (display {})", d.display_id()),
            kind: AudioKind::System,
        })
        .collect())
}
struct Handler {
    sender: SyncSender<CMSampleBuffer>,
    failed: Arc<AtomicBool>,
}
impl SCStreamOutputTrait for Handler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, kind: SCStreamOutputType) {
        if kind != SCStreamOutputType::Audio || self.failed.load(Ordering::Acquire) {
            return;
        }
        // Bound retained native memory before queueing. At most two 50 ms
        // stereo f32 samples remain retained. No PCM conversion or Opus here.
        if sample.num_samples() <= 0 || sample.num_samples() > 2400 {
            self.failed.store(true, Ordering::Release);
            return;
        }
        let Some(format) = sample.format_description() else {
            self.failed.store(true, Ordering::Release);
            return;
        };
        if !valid_format(&format) || self.sender.try_send(sample).is_err() {
            self.failed.store(true, Ordering::Release);
        }
    }
}
fn valid_format(format: &apple_cf::cm::CMFormatDescription) -> bool {
    format.is_pcm()
        && format.audio_sample_rate() == Some(48_000.0)
        && matches!(format.audio_channel_count(), Some(1 | 2))
        && format.audio_bits_per_channel() == Some(32)
        && format.audio_is_float()
        && !format.audio_is_big_endian()
        && matches!(format.audio_bytes_per_frame(), Some(4 | 8))
}

pub struct MacSystemAudio {
    stream: SCStream,
    receiver: Receiver<CMSampleBuffer>,
    failed: Arc<AtomicBool>,
    encoder: AudioEncoder,
    format: Format,
    pending: Vec<f32>,
}
impl MacSystemAudio {
    pub fn open(
        id: &str,
        format: Format,
        startup: &vnilink::BackendStartup,
    ) -> Result<Self, vnilink::Error> {
        startup.check()?;
        let id = selected_id(id)?;
        if !permission() {
            return Err(vnilink::Error::Unsupported("system audio permission"));
        }
        let content = SCShareableContent::get()
            .map_err(|_| vnilink::Error::Unsupported("system audio display"))?;
        startup.check()?;
        let displays = content.displays();
        let display = displays
            .iter()
            .find(|d| d.display_id() == id)
            .ok_or(vnilink::Error::Unsupported("selected system audio display"))?;
        let filter = SCContentFilter::create()
            .with_display(display)
            .with_excluding_windows(&[])
            .build()
            .map_err(|_| vnilink::Error::Unsupported("system audio filter"))?;
        let config = configuration(format);
        let (sender, receiver) = sync_channel(2);
        let failed = Arc::new(AtomicBool::new(false));
        let error = failed.clone();
        let delegate = StreamCallbacks::new().on_error(move |_| {
            error.store(true, Ordering::Release);
        });
        let mut stream = SCStream::new_with_delegate(&filter, &config, delegate)
            .map_err(|_| vnilink::Error::Unsupported("system audio stream"))?;
        stream
            .add_output_handler(
                Handler {
                    sender,
                    failed: failed.clone(),
                },
                SCStreamOutputType::Audio,
            )
            .map_err(|_| vnilink::Error::Unsupported("system audio handler"))?;
        let capture = Self {
            stream,
            receiver,
            failed,
            encoder: AudioEncoder::new(format, false)
                .map_err(|_| vnilink::Error::Unsupported("audio codec"))?,
            format,
            pending: Vec::with_capacity(6720),
        };
        startup.check()?;
        capture
            .stream
            .start_capture()
            .map_err(|_| vnilink::Error::Unsupported("system audio capture"))?;
        startup.check()?; // Drop owns stop even when OS startup finishes late.
        Ok(capture)
    }
    pub fn next_packet(&mut self) -> Result<Option<AudioPacket>, vnilink::Error> {
        if self.failed.load(Ordering::Acquire) {
            return Err(vnilink::Error::Unsupported("system audio capture ended"));
        }
        let required = FRAME_SAMPLES * self.format.channels();
        if self.pending.len() < required {
            match self.receiver.try_recv() {
                Ok(sample) => {
                    let description = sample
                        .format_description()
                        .ok_or(vnilink::Error::Malformed)?;
                    if !valid_format(&description) {
                        return Err(vnilink::Error::Malformed);
                    }
                    let count = usize::try_from(sample.num_samples())
                        .map_err(|_| vnilink::Error::Malformed)?;
                    if count == 0 || count > 2400 {
                        return Err(vnilink::Error::Malformed);
                    }
                    let channels = description
                        .audio_channel_count()
                        .ok_or(vnilink::Error::Malformed)?
                        as usize;
                    let buffers = sample
                        .audio_buffer_list()
                        .map_err(|_| vnilink::Error::Unsupported("system audio PCM"))?;
                    let planes: Vec<_> = buffers
                        .iter()
                        .map(|b| (b.number_channels as usize, b.data()))
                        .collect();
                    append_pcm(
                        &planes,
                        count,
                        channels,
                        self.format.channels(),
                        &mut self.pending,
                    )?;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(None),
                Err(_) => return Err(vnilink::Error::Unsupported("system audio capture ended")),
            }
        }
        if self.pending.len() < required {
            return Ok(None);
        }
        let packet = self
            .encoder
            .encode(&self.pending[..required])
            .map_err(|_| vnilink::Error::Unsupported("audio codec"))?;
        self.pending.drain(..required);
        Ok(Some(packet))
    }
}
impl Drop for MacSystemAudio {
    fn drop(&mut self) {
        self.failed.store(true, Ordering::Release);
        let _ = self.stream.stop_capture();
    }
}
fn configuration(format: Format) -> SCStreamConfiguration {
    // No Screen handler is registered; small video configuration avoids a full
    // desktop video pipeline for the independent audio service.
    SCStreamConfiguration::new()
        .with_width(2)
        .with_height(2)
        .with_captures_audio(true)
        .with_sample_rate(48_000)
        .with_channel_count(format.channels() as i32)
        .with_excludes_current_process_audio(true)
}
fn append_pcm(
    planes: &[(usize, &[u8])],
    count: usize,
    input: usize,
    output: usize,
    pending: &mut Vec<f32>,
) -> Result<(), vnilink::Error> {
    if count == 0
        || count > 2400
        || !(1..=2).contains(&input)
        || !(1..=2).contains(&output)
        || pending.len() + count * output > 6720
    {
        return Err(vnilink::Error::Malformed);
    }
    let interleaved =
        planes.len() == 1 && planes[0].0 == input && planes[0].1.len() == count * input * 4;
    let planar = planes.len() == input
        && planes
            .iter()
            .all(|(channels, bytes)| *channels == 1 && bytes.len() == count * 4);
    if !interleaved && !planar {
        return Err(vnilink::Error::Malformed);
    }
    let read = |frame: usize, channel: usize| -> f32 {
        let (bytes, offset) = if interleaved {
            (planes[0].1, (frame * input + channel) * 4)
        } else {
            (planes[channel].1, frame * 4)
        };
        let value = f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        if value.is_finite() {
            value.clamp(-1.0, 1.0)
        } else {
            0.0
        }
    };
    for frame in 0..count {
        let left = read(frame, 0);
        let right = read(frame, input - 1);
        if output == 1 {
            pending.push((left + right) * 0.5);
        } else {
            pending.extend_from_slice(&[left, right]);
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn planar_and_interleaved_copy_match_and_mix_correctly() {
        let left: Vec<u8> = [0.25f32, -0.5]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        let right: Vec<u8> = [0.75f32, 0.5]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        let interleaved: Vec<u8> = [0.25f32, 0.75, -0.5, 0.5]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        let mut a = Vec::new();
        let mut b = Vec::new();
        append_pcm(&[(1, &left), (1, &right)], 2, 2, 2, &mut a).unwrap();
        append_pcm(&[(2, &interleaved)], 2, 2, 2, &mut b).unwrap();
        assert_eq!(a, [0.25, 0.75, -0.5, 0.5]);
        assert_eq!(a, b);
        let mut mono = Vec::new();
        append_pcm(&[(1, &left), (1, &right)], 2, 2, 1, &mut mono).unwrap();
        assert_eq!(mono, [0.5, 0.0]);
        assert!(append_pcm(&[(2, &interleaved[..12])], 2, 2, 2, &mut b).is_err());
        assert_eq!(b, a); // Malformed layouts cannot append a partial block.
    }
    #[test]
    fn cancelled_startup_does_not_enumerate_or_open_native_capture() {
        let stop = Arc::new(AtomicBool::new(true));
        let startup = vnilink::BackendStartup::with_stop(stop, std::time::Duration::from_secs(5));
        assert!(matches!(
            MacSystemAudio::open("sck-system:42", Format::Stereo, &startup),
            Err(vnilink::Error::NoSession)
        ));
    }
    /// Opt-in only: play a test tone in another process, with already granted
    /// Screen Recording permission. No PCM/content is written to disk or logs.
    #[test]
    #[ignore = "requires explicit source approval and existing Mac screen/audio permission"]
    fn explicit_system_audio_delivers_decodable_packets_and_stops() {
        let id = std::env::var("VNILINK_MAC_SYSTEM_AUDIO_ID").expect("explicit source ID");
        let startup = vnilink::BackendStartup::new(std::time::Duration::from_secs(5));
        let mut capture = MacSystemAudio::open(&id, Format::Stereo, &startup).unwrap();
        let mut decoder = opus::Decoder::new(48000, opus::Channels::Stereo).unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut count = 0;
        let mut energy = 0.0f64;
        while std::time::Instant::now() < until {
            if let Some(packet) = capture.next_packet().unwrap() {
                let mut pcm = [0.0f32; 1920];
                assert_eq!(
                    decoder
                        .decode_float(&packet.payload, &mut pcm, false)
                        .unwrap(),
                    960
                );
                energy += pcm.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>();
                count += 1;
            } else {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        assert!(count >= 100);
        assert!(energy / (count as f64 * 1920.0) > 1e-5);
        let closing = std::time::Instant::now();
        drop(capture);
        assert!(closing.elapsed() < std::time::Duration::from_secs(2));
    }
    #[test]
    fn configuration_and_ids_do_not_activate_capture() {
        let config = configuration(Format::Stereo);
        assert!(config.captures_audio());
        assert!(config.excludes_current_process_audio());
        assert_eq!(config.sample_rate(), 48000);
        assert_eq!(config.channel_count(), 2);
        assert_eq!(selected_id("sck-system:42").unwrap(), 42);
        for id in ["sck-system:0", "sck-system:042", "42", "sck-system:-1"] {
            assert!(selected_id(id).is_err());
        }
    }
}
