//! One encoded H.264 frame. The payload is screen content: do not log it.

use std::fmt;
use std::time::Instant;

use crate::error::Error;

use super::MAX_VIDEO_FRAME;

/// Header before the payload: codec, flags, timestamp, source display id,
/// source topology revision, the host's capture-to-send time, and the
/// picture's number in its stream. A frame that says where it changed has
/// its regions between this and the payload.
pub(crate) const VIDEO_HEADER: usize = 34;

/// The most regions a frame carries; a picture changed in more places says
/// none, as if all of it changed.
pub const MAX_REGIONS: usize = 64;

/// The bytes the regions may add after [`VIDEO_HEADER`]: a count, then each.
const MAX_REGION_BYTES: usize = 1 + MAX_REGIONS * 8;
use super::VideoCodec;

/// One encoded H.264 frame. Construct it with [`EncodedFrame::h264`].
///
/// Fields stay visible so a caller can replace the payload. [`crate::Core::publish_video`]
/// checks the payload again.
///
/// `display_id`/`revision` name the captured source: the topology display
/// id and the topology revision the producer captured under. Zero means
/// unstamped (synthetic, non-display, or whole-desktop frames that name no
/// single display). Desktop producers always stamp via
/// [`EncodedFrame::with_source`]; publish backstops a zero revision from
/// the live topology but never invents a display id, so viewers map
/// pointer input by attested source id (fencing unknown sources) instead
/// of guessing from picture dimensions.
#[derive(Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    pub codec: VideoCodec,
    pub keyframe: bool,
    pub timestamp_us: u64,
    pub payload: Vec<u8>,
    pub display_id: u64,
    pub revision: u64,
    /// What the picture is to the stream: see [`FrameRole`].
    pub role: FrameRole,
    /// Where the picture differs from the one before it in the stream, in
    /// its own pixels; `None` when the host cannot say, as if all of it did.
    /// Only a [`FrameRole::Changed`] picture says.
    pub regions: Option<Vec<Region>>,
    /// How long the frame took to get here; telemetry, not content.
    pub timing: FrameTiming,
}

/// A part of a picture, in its pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Region {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// What a frame's picture is to its stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrameRole {
    /// A picture of the screen as it is now; the usual frame.
    #[default]
    Changed,
    /// The same screen again: an idle keepalive or a refinement of the
    /// last picture. A viewer showing a [`FrameRole::Still`] keeps it.
    Unchanged,
    /// The still screen once at full colour, AV1 4:4:4, outside the
    /// stream's codec: shown until the screen changes, never decoded with
    /// the stream, recorded or counted as its keyframe.
    Still,
}

/// Header flag bits after the keyframe bit.
const FLAG_KEYFRAME: u8 = 1;
const FLAG_STILL: u8 = 2;
const FLAG_UNCHANGED: u8 = 4;
const FLAG_REGIONS: u8 = 8;

/// Where a frame's time went, for the latency of each stage. Timing is not
/// part of a frame's identity: frames with the same picture compare equal
/// whatever their timing.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameTiming {
    /// When the host captured the picture; on the host only.
    pub captured_at: Option<Instant>,
    /// Capture to send on the host, in microseconds, as received.
    pub host_us: u32,
    /// Time spent queued for the window on the viewer, in microseconds.
    pub queued_us: u32,
}

impl PartialEq for FrameTiming {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for FrameTiming {}

/// Microseconds since `at`, saturated to the header's 32 bits (71 minutes).
pub(crate) fn micros_since(at: Instant) -> u32 {
    u32::try_from(at.elapsed().as_micros()).unwrap_or(u32::MAX)
}

impl EncodedFrame {
    pub fn h264(keyframe: bool, timestamp_us: u64, payload: Vec<u8>) -> Result<Self, Error> {
        Self::new(VideoCodec::H264, keyframe, timestamp_us, payload)
    }

    pub fn new(
        codec: VideoCodec,
        keyframe: bool,
        timestamp_us: u64,
        payload: Vec<u8>,
    ) -> Result<Self, Error> {
        validate_payload(&payload)?;
        Ok(Self {
            codec,
            keyframe,
            timestamp_us,
            payload,
            display_id: 0,
            revision: 0,
            role: FrameRole::Changed,
            regions: None,
            timing: FrameTiming::default(),
        })
    }

    /// Says where the picture changed; more than [`MAX_REGIONS`] say
    /// nothing, as if all of it did.
    pub fn with_regions(mut self, regions: Option<Vec<Region>>) -> Self {
        self.regions = regions.filter(|regions| regions.len() <= MAX_REGIONS);
        self
    }

    /// A still picture: an AV1 4:4:4 keyframe of a still screen.
    pub fn still(timestamp_us: u64, payload: Vec<u8>) -> Result<Self, Error> {
        let mut frame = Self::new(VideoCodec::Av1, true, timestamp_us, payload)?;
        frame.role = FrameRole::Still;
        Ok(frame)
    }

    /// Marks the picture as the same screen again.
    pub fn unchanged(mut self) -> Self {
        if self.role == FrameRole::Changed {
            self.role = FrameRole::Unchanged;
        }
        self
    }

    /// Whether this frame belongs to a stream agreed as `codec` (a wire id):
    /// a picture of that codec, or a still picture where stills were agreed.
    pub(crate) fn belongs_to(&self, codec: u8, stills: bool) -> bool {
        if self.role == FrameRole::Still {
            stills
        } else {
            Some(self.codec) == VideoCodec::from_wire_id(codec)
        }
    }

    /// Stamps when the host captured the picture this frame encodes.
    pub fn with_captured_at(mut self, at: Instant) -> Self {
        self.timing.captured_at = Some(at);
        self
    }

    /// Stamps the captured source: topology display id and revision.
    pub fn with_source(mut self, display_id: u64, revision: u64) -> Self {
        self.display_id = display_id;
        self.revision = revision;
        self
    }

    pub(crate) fn check(&self) -> Result<(), Error> {
        validate_payload(&self.payload)
    }
}

impl fmt::Debug for EncodedFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncodedFrame")
            .field("keyframe", &self.keyframe)
            .field("timestamp_us", &self.timestamp_us)
            .field("display_id", &self.display_id)
            .field("revision", &self.revision)
            .field("role", &self.role)
            .field("regions", &self.regions.as_ref().map(Vec::len))
            .field("timing", &self.timing)
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

fn validate_payload(payload: &[u8]) -> Result<(), Error> {
    if payload.is_empty() {
        return Err(Error::InvalidSetting("encoded frame is empty"));
    }
    if payload.len() > MAX_VIDEO_FRAME {
        return Err(Error::InvalidSetting("encoded frame is larger than 1 MiB"));
    }
    Ok(())
}
pub(crate) fn check_video_len(len: usize) -> Result<(), Error> {
    if len < VIDEO_HEADER || len > VIDEO_HEADER + MAX_REGION_BYTES + MAX_VIDEO_FRAME {
        Err(Error::Malformed)
    } else {
        Ok(())
    }
}

/// A picture as it crosses the wire: the frame, and its number among the
/// stream pictures its host sent.
///
/// The host numbers each stream picture it puts on the link, by datagrams
/// or on the stream, one after another; a picture it drops before sending
/// takes no number. A still repeats the number of the stream picture before
/// it, so a lost still leaves no gap. The number wraps.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct VideoBody {
    pub(crate) sequence: u32,
    pub(crate) frame: EncodedFrame,
}

pub(crate) fn encode_video_body(frame: &EncodedFrame, sequence: u32) -> Vec<u8> {
    let regions = frame
        .regions
        .as_ref()
        .filter(|regions| frame.role == FrameRole::Changed && regions.len() <= MAX_REGIONS);
    let mut body = Vec::with_capacity(VIDEO_HEADER + MAX_REGION_BYTES + frame.payload.len());
    body.push(frame.codec.wire_id());
    body.push(
        if frame.keyframe { FLAG_KEYFRAME } else { 0 }
            | if regions.is_some() { FLAG_REGIONS } else { 0 }
            | match frame.role {
                FrameRole::Changed => 0,
                FrameRole::Unchanged => FLAG_UNCHANGED,
                FrameRole::Still => FLAG_STILL,
            },
    );
    body.extend_from_slice(&frame.timestamp_us.to_be_bytes());
    body.extend_from_slice(&frame.display_id.to_be_bytes());
    body.extend_from_slice(&frame.revision.to_be_bytes());
    // Measured as the frame is put on the wire; a frame passed on keeps
    // the time it arrived with.
    let host_us = frame
        .timing
        .captured_at
        .map_or(frame.timing.host_us, micros_since);
    body.extend_from_slice(&host_us.to_be_bytes());
    body.extend_from_slice(&sequence.to_be_bytes());
    if let Some(regions) = regions {
        body.push(regions.len() as u8);
        for region in regions {
            for value in [region.x, region.y, region.width, region.height] {
                body.extend_from_slice(&value.to_be_bytes());
            }
        }
    }
    body.extend_from_slice(&frame.payload);
    body
}

pub(crate) fn decode_video_body(body: &[u8]) -> Result<VideoBody, Error> {
    check_video_len(body.len())?;
    let codec = VideoCodec::from_wire_id(body[0]).ok_or(Error::Unsupported("this video codec"))?;
    if codec.wire_id() != body[0] {
        return Err(Error::Unsupported("this video codec"));
    }
    let timestamp_us = u64::from_be_bytes(body[2..10].try_into().expect("8 bytes"));
    let display_id = u64::from_be_bytes(body[10..18].try_into().expect("8 bytes"));
    let revision = u64::from_be_bytes(body[18..26].try_into().expect("8 bytes"));
    let host_us = u32::from_be_bytes(body[26..30].try_into().expect("4 bytes"));
    let sequence = u32::from_be_bytes(body[30..34].try_into().expect("4 bytes"));
    let flags = body[1];
    let keyframe = flags & FLAG_KEYFRAME != 0;
    let role = match (flags & FLAG_STILL != 0, flags & FLAG_UNCHANGED != 0) {
        (false, false) => FrameRole::Changed,
        (false, true) => FrameRole::Unchanged,
        // A still is one AV1 keyframe and nothing else.
        (true, false) if keyframe && codec == VideoCodec::Av1 => FrameRole::Still,
        _ => return Err(Error::Malformed),
    };
    let mut payload = &body[VIDEO_HEADER..];
    let mut regions = None;
    if flags & FLAG_REGIONS != 0 {
        // Only a changed picture says where it changed.
        if role != FrameRole::Changed {
            return Err(Error::Malformed);
        }
        let (&count, rest) = payload.split_first().ok_or(Error::Malformed)?;
        let count = usize::from(count);
        if count > MAX_REGIONS || rest.len() < count * 8 {
            return Err(Error::Malformed);
        }
        let value = |at: usize| u16::from_be_bytes([rest[at], rest[at + 1]]);
        regions = Some(
            (0..count)
                .map(|index| Region {
                    x: value(index * 8),
                    y: value(index * 8 + 2),
                    width: value(index * 8 + 4),
                    height: value(index * 8 + 6),
                })
                .collect(),
        );
        payload = &rest[count * 8..];
    }
    let mut frame = EncodedFrame::new(codec, keyframe, timestamp_us, payload.to_vec())
        .map(|frame| frame.with_source(display_id, revision))
        .map_err(|_| Error::Malformed)?;
    frame.role = role;
    frame.regions = regions;
    frame.timing.host_us = host_us;
    Ok(VideoBody { sequence, frame })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::VideoQueue;
    use std::time::Duration;

    fn encode(frame: &EncodedFrame) -> Vec<u8> {
        encode_video_body(frame, 0)
    }

    fn decode(body: &[u8]) -> Result<EncodedFrame, Error> {
        decode_video_body(body).map(|body| body.frame)
    }

    #[test]
    fn a_frame_carries_its_capture_to_send_time_and_its_queued_time() {
        let captured = Instant::now() - Duration::from_millis(40);
        let frame = EncodedFrame::h264(true, 1, vec![0x65])
            .unwrap()
            .with_captured_at(captured);
        let sent = decode(&encode(&frame)).unwrap();
        assert!(
            (40_000..10_000_000).contains(&sent.timing.host_us),
            "capture to send was {} µs",
            sent.timing.host_us
        );
        assert_eq!(
            sent.timing.captured_at, None,
            "the capture instant stays on the host"
        );
        // A received frame passed on keeps the time it arrived with.
        let passed = decode(&encode(&sent)).unwrap();
        assert_eq!(passed.timing.host_us, sent.timing.host_us);
        // Without a capture instant the time is unknown: zero.
        let unknown = EncodedFrame::h264(true, 1, vec![0x65]).unwrap();
        assert_eq!(decode(&encode(&unknown)).unwrap().timing.host_us, 0);

        let mut queue = VideoQueue::new();
        queue.push_at(sent, Instant::now() - Duration::from_millis(25));
        let popped = queue.pop().unwrap();
        assert!(
            popped.timing.queued_us >= 25_000,
            "queued {} µs",
            popped.timing.queued_us
        );
    }

    /// A changed picture says where it changed; more places than a frame
    /// carries, or a picture that is not a change, say nothing.
    #[test]
    fn a_changed_frame_carries_where_it_changed() {
        use crate::{Region, MAX_REGIONS};
        let clock = Region {
            x: 1800,
            y: 1040,
            width: 96,
            height: 24,
        };
        let frame = EncodedFrame::h264(false, 5, vec![0x41, 2])
            .unwrap()
            .with_regions(Some(vec![clock]));
        let back = decode(&encode(&frame)).unwrap();
        assert_eq!(back.regions, Some(vec![clock]));
        assert_eq!(back.payload, vec![0x41, 2]);
        // None changed: an empty list is a statement too.
        let none = frame.clone().with_regions(Some(Vec::new()));
        assert_eq!(decode(&encode(&none)).unwrap().regions, Some(Vec::new()));
        let many = frame
            .clone()
            .with_regions(Some(vec![clock; MAX_REGIONS + 1]));
        assert_eq!(many.regions, None);
        assert_eq!(decode(&encode(&many)).unwrap().regions, None);
        // A repeat of the screen says nothing of regions, whatever it holds.
        let repeat = frame.clone().unchanged();
        assert_eq!(decode(&encode(&repeat)).unwrap().regions, None);
        // Regions on a repeat, and a count past the limit or past the body, are
        // malformed.
        let mut body = encode(&frame);
        body[1] |= 4;
        assert!(matches!(decode(&body), Err(Error::Malformed)));
        let mut body = encode(&frame);
        body[VIDEO_HEADER] = (MAX_REGIONS + 1) as u8;
        assert!(matches!(decode(&body), Err(Error::Malformed)));
        let mut body = encode(&frame);
        body[VIDEO_HEADER] = 2;
        assert!(matches!(decode(&body), Err(Error::Malformed)));
    }

    #[test]
    fn h264_frames_round_trip_and_reject_a_bad_payload() {
        assert!(matches!(
            EncodedFrame::h264(true, 1, Vec::new()),
            Err(Error::InvalidSetting("encoded frame is empty"))
        ));
        assert!(matches!(
            EncodedFrame::h264(true, 1, vec![0; MAX_VIDEO_FRAME + 1]),
            Err(Error::InvalidSetting("encoded frame is larger than 1 MiB"))
        ));
        let small = EncodedFrame::h264(false, u64::MAX, vec![0x7F]).unwrap();
        assert_eq!(decode(&encode(&small)).unwrap(), small);
        let stamped = EncodedFrame::h264(true, 9, vec![0x65, 1])
            .unwrap()
            .with_source(3, 7);
        let back = decode(&encode(&stamped)).unwrap();
        assert_eq!(back, stamped);
        assert_eq!((back.display_id, back.revision), (3, 7));

        let mut flags = encode(&EncodedFrame::h264(true, 7, vec![4]).unwrap());
        // Bits past the keyframe, role and region bits are ignored.
        flags[1] = 0xF1;
        assert!(decode(&flags).unwrap().keyframe);
        flags[1] = 0xF0;
        assert!(!decode(&flags).unwrap().keyframe);

        let mut unknown = vec![0; VIDEO_HEADER + 1];
        unknown[0] = 9;
        assert!(matches!(
            decode(&unknown),
            Err(Error::Unsupported("this video codec"))
        ));
        assert!(matches!(decode(&[1, 0]), Err(Error::Malformed)));
        assert!(matches!(
            decode(&[1, 1, 0, 0, 0, 0, 0, 0, 0, 0]),
            Err(Error::Malformed)
        ));
        // The old 10-byte header is too short to carry a source stamp.
        assert!(matches!(
            decode(&[1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
            Err(Error::Malformed)
        ));

        let mut mutated = EncodedFrame::h264(true, 1, vec![0xAB, 0xCD]).unwrap();
        let rendered = format!("{mutated:?}");
        assert!(rendered.contains("payload_len"));
        assert!(!rendered.contains("171"));
        assert!(!rendered.contains("205"));
        mutated.payload.clear();
        assert!(matches!(
            mutated.check(),
            Err(Error::InvalidSetting("encoded frame is empty"))
        ));
        mutated.payload.resize(MAX_VIDEO_FRAME + 1, 0);
        assert!(matches!(
            mutated.check(),
            Err(Error::InvalidSetting("encoded frame is larger than 1 MiB"))
        ));
    }

    /// A frame's role crosses the wire in its flags; a still is only ever an
    /// AV1 keyframe, and no frame is both a still and unchanged.
    #[test]
    fn a_frames_role_round_trips_and_a_malformed_still_is_refused() {
        use crate::{FrameRole, VideoCodec};
        let changed = EncodedFrame::h264(false, 1, vec![9; 40]).unwrap();
        let unchanged = changed.clone().unchanged();
        let still = EncodedFrame::still(3, vec![7; 40]).unwrap();
        for frame in [&changed, &unchanged, &still] {
            let back = decode(&encode(frame)).unwrap();
            assert_eq!(back.role, frame.role);
            assert_eq!(back.keyframe, frame.keyframe);
            assert_eq!(back.codec, frame.codec);
        }
        assert_eq!(still.codec, VideoCodec::Av1);
        assert_eq!(unchanged.role, FrameRole::Unchanged);
        // A still stays a still when marked unchanged.
        assert_eq!(still.clone().unchanged().role, FrameRole::Still);
        let flags = |frame: &EncodedFrame, flags: u8| {
            let mut body = encode(frame);
            body[1] = flags;
            decode(&body)
        };
        // Still flag on a predicted frame, on H.264, or with the unchanged flag.
        assert!(flags(&still, 2).is_err());
        assert!(flags(&EncodedFrame::h264(true, 1, vec![9; 40]).unwrap(), 3).is_err());
        assert!(flags(&still, 7).is_err());
    }

    /// A picture's number crosses the wire beside the frame, up to the last
    /// before it wraps, and a header without room for it is refused.
    #[test]
    fn a_pictures_number_round_trips_through_its_wraparound() {
        let frame = EncodedFrame::h264(false, 4, vec![0x41; 8])
            .unwrap()
            .with_regions(Some(vec![crate::Region::default()]));
        for sequence in [0, 1, 0x0102_0304, u32::MAX - 1, u32::MAX] {
            let back = decode_video_body(&encode_video_body(&frame, sequence)).unwrap();
            assert_eq!(
                back,
                VideoBody {
                    sequence,
                    frame: frame.clone()
                },
                "picture {sequence} comes back as sent, its regions after the number"
            );
        }
        let still = EncodedFrame::still(9, vec![7; 8]).unwrap();
        assert_eq!(
            decode_video_body(&encode_video_body(&still, u32::MAX))
                .unwrap()
                .sequence,
            u32::MAX,
            "a still carries the number it repeats"
        );
        let mut short = encode_video_body(&frame, 7);
        short.truncate(VIDEO_HEADER - 1);
        assert!(
            matches!(decode_video_body(&short), Err(Error::Malformed)),
            "a header too short for the number is malformed"
        );
    }
}
