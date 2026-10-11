//! Narrow Matroska/AVC remuxer. No capture, decode, UI or filesystem selection.
//! Each complete Cluster is independently recoverable after a truncated tail.
use crate::parse_annex_b;
use std::io::{self, Seek, SeekFrom, Write};
use vnilink::EncodedFrame;

#[derive(Debug, thiserror::Error)]
pub enum RecordingError {
    #[error("recording source unavailable")]
    Source(#[source] vnilink::Error),
    #[error("recording media or clock is invalid")]
    InvalidMedia,
    #[error("recording format changed; start a new segment")]
    FormatChanged,
    #[error("recording resource limit reached")]
    Limit,
    #[error("recording storage failed")]
    Storage(#[source] io::Error),
}
impl From<io::Error> for RecordingError {
    fn from(e: io::Error) -> Self {
        Self::Storage(e)
    }
}
#[derive(Debug, Clone, Copy)]
pub struct RecordingLimits {
    pub max_bytes: u64,
    pub max_duration_ms: u64,
}
impl RecordingLimits {
    pub fn validate(self) -> Result<(), RecordingError> {
        if !(1_048_576..=1_099_511_627_776).contains(&self.max_bytes)
            || !(1_000..=86_400_000).contains(&self.max_duration_ms)
        {
            return Err(RecordingError::Limit);
        }
        Ok(())
    }
}
/// H.264 without B frames, one display/codec epoch per segment. Cursor pixels
/// are preserved only when already embedded; metadata is not silently burned in.
pub struct MatroskaRecorder<W: Write + Seek> {
    writer: W,
    limits: RecordingLimits,
    position: u64,
    duration_position: u64,
    last_time: Option<u64>,
    duration: u64,
    sps: Vec<u8>,
    pps: Vec<u8>,
    failed: bool,
    frames: u64,
    cues: Vec<(u64, u64)>,
    segment_start: u64,
}
impl<W: Write + Seek> MatroskaRecorder<W> {
    /// Readiness requires a real self-contained IDR. Geometry comes from the
    /// selected owner; changed SPS/PPS ends the segment instead of corrupting it.
    pub fn start(
        writer: W,
        width: u16,
        height: u16,
        first: &EncodedFrame,
        limits: RecordingLimits,
    ) -> Result<Self, RecordingError> {
        limits.validate()?;
        if width == 0 || height == 0 || !first.keyframe {
            return Err(RecordingError::InvalidMedia);
        }
        let unit = parse_annex_b(&first.payload).map_err(|_| RecordingError::InvalidMedia)?;
        if !unit.has_idr() {
            return Err(RecordingError::InvalidMedia);
        }
        let sps = unit
            .units()
            .iter()
            .find(|u| u.nal_type == 7)
            .ok_or(RecordingError::InvalidMedia)?
            .bytes()
            .to_vec();
        let pps = unit
            .units()
            .iter()
            .find(|u| u.nal_type == 8)
            .ok_or(RecordingError::InvalidMedia)?
            .bytes()
            .to_vec();
        if sps.len() < 4 || sps.len() > 65535 || pps.is_empty() || pps.len() > 65535 {
            return Err(RecordingError::InvalidMedia);
        }
        let actual = crate::h264_picture_size(&sps).map_err(|_| RecordingError::InvalidMedia)?;
        if actual != (width, height) {
            return Err(RecordingError::InvalidMedia);
        }
        // AVCDecoderConfigurationRecord, four-byte NAL lengths.
        let mut avcc = vec![1, sps[1], sps[2], sps[3], 0xff, 0xe1];
        avcc.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        avcc.extend_from_slice(&sps);
        avcc.push(1);
        avcc.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        avcc.extend_from_slice(&pps);
        let mut out = Self {
            writer,
            limits,
            position: 0,
            duration_position: 0,
            last_time: None,
            duration: 0,
            sps,
            pps,
            failed: false,
            frames: 0,
            cues: Vec::new(),
            segment_start: 0,
        };
        if out.writer.stream_position()? != 0 {
            return Err(RecordingError::InvalidMedia);
        }
        let mut ebml = Vec::new();
        uint(&mut ebml, 0x4286, 1);
        uint(&mut ebml, 0x42f7, 1);
        uint(&mut ebml, 0x42f2, 4);
        uint(&mut ebml, 0x42f3, 8);
        element(&mut ebml, 0x4282, b"matroska");
        uint(&mut ebml, 0x4287, 4);
        uint(&mut ebml, 0x4285, 2);
        out.emit(&master(0x1a45dfa3, &ebml))?;
        out.emit(&[
            0x18, 0x53, 0x80, 0x67, 0x01, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        ])?;
        out.segment_start = out.position;
        let mut info = Vec::new();
        uint(&mut info, 0x2ad7b1, 1_000_000);
        element(&mut info, 0x4d80, b"VniLink");
        element(&mut info, 0x5741, b"VniLink");
        let relative = info.len();
        element(&mut info, 0x4489, &0f64.to_be_bytes());
        let info_element = master(0x1549a966, &info);
        out.duration_position =
            out.position + (info_element.len() - info.len() + relative + 3) as u64;
        out.emit(&info_element)?;
        let mut video = Vec::new();
        uint(&mut video, 0xb0, u64::from(width));
        uint(&mut video, 0xba, u64::from(height));
        let mut track = Vec::new();
        uint(&mut track, 0xd7, 1);
        uint(&mut track, 0x73c5, 1);
        uint(&mut track, 0x83, 1);
        uint(&mut track, 0x9c, 0);
        element(&mut track, 0x86, b"V_MPEG4/ISO/AVC");
        element(&mut track, 0x63a2, &avcc);
        track.extend(master(0xe0, &video));
        out.emit(&master(0x1654ae6b, &master(0xae, &track)))?;
        out.push_video(first, 0)?;
        Ok(out)
    }
    fn emit(&mut self, bytes: &[u8]) -> Result<(), RecordingError> {
        if self.failed {
            return Err(RecordingError::InvalidMedia);
        }
        let next = self
            .position
            .checked_add(bytes.len() as u64)
            .ok_or(RecordingError::Limit)?;
        if next > self.limits.max_bytes {
            return Err(RecordingError::Limit);
        }
        if let Err(error) = self.writer.write_all(bytes) {
            self.failed = true;
            return Err(error.into());
        }
        self.position = next;
        Ok(())
    }
    /// Caller supplies a recording-relative monotonic timeline, never a raw
    /// subtraction between clocks on different computers. Frames are not queued.
    pub fn push_video(&mut self, frame: &EncodedFrame, time_ms: u64) -> Result<(), RecordingError> {
        if self.failed
            || time_ms > self.limits.max_duration_ms
            || self.last_time.is_some_and(|last| time_ms < last)
        {
            return Err(RecordingError::Limit);
        }
        let unit = parse_annex_b(&frame.payload).map_err(|_| RecordingError::InvalidMedia)?;
        if unit.has_idr() != frame.keyframe {
            return Err(RecordingError::InvalidMedia);
        }
        for nal in unit.units() {
            if nal.nal_type == 7 && nal.bytes() != self.sps
                || nal.nal_type == 8 && nal.bytes() != self.pps
            {
                return Err(RecordingError::FormatChanged);
            }
        }
        let mut block = vec![0x81, 0, 0, if frame.keyframe { 0x80 } else { 0 }];
        let mut picture = false;
        for nal in unit.units() {
            if matches!(nal.nal_type, 7 | 8 | 9) {
                continue;
            }
            if matches!(nal.nal_type, 1 | 5) {
                picture = true;
            }
            block.extend_from_slice(&(nal.bytes().len() as u32).to_be_bytes());
            block.extend_from_slice(nal.bytes());
        }
        if !picture {
            return Err(RecordingError::InvalidMedia);
        }
        let mut cluster = Vec::new();
        uint(&mut cluster, 0xe7, time_ms);
        element(&mut cluster, 0xa3, &block);
        let position = self.position - self.segment_start;
        // Reserve final cue bytes as well, rather than reaching the storage
        // ceiling with a file that cannot be finalized.
        if self.cues.len() >= 100_000
            || self.position + cluster.len() as u64 + self.cues.len() as u64 * 40 + 128
                > self.limits.max_bytes
        {
            return Err(RecordingError::Limit);
        }
        self.emit(&master(0x1f43b675, &cluster))?;
        if frame.keyframe {
            self.cues.push((time_ms, position));
        }
        self.last_time = Some(time_ms);
        self.duration = time_ms;
        self.frames += 1;
        Ok(())
    }
    pub fn frames(&self) -> u64 {
        self.frames
    }
    pub fn bytes(&self) -> u64 {
        self.position
    }
    pub fn finish(mut self) -> Result<W, RecordingError> {
        if self.failed || self.frames == 0 {
            return Err(RecordingError::InvalidMedia);
        }
        let mut cues = Vec::new();
        for (time, position) in &self.cues {
            let mut seek = Vec::new();
            uint(&mut seek, 0xf7, 1);
            uint(&mut seek, 0xf1, *position);
            let mut cue = Vec::new();
            uint(&mut cue, 0xb3, *time);
            cue.extend(master(0xb7, &seek));
            cues.extend(master(0xbb, &cue));
        }
        self.emit(&master(0x1c53bb6b, &cues))?;
        self.writer.seek(SeekFrom::Start(self.duration_position))?;
        self.writer
            .write_all(&(self.duration as f64).to_be_bytes())?;
        self.writer.seek(SeekFrom::Start(self.position))?;
        self.writer.flush()?;
        Ok(self.writer)
    }
}
fn size(value: usize) -> Vec<u8> {
    for width in 1..=8 {
        if (value as u64) < (1u64 << (7 * width)) - 1 {
            let encoded = (value as u64) | (1u64 << (7 * width));
            return encoded.to_be_bytes()[8 - width..].to_vec();
        }
    }
    unreachable!("bounded media sizes fit EBML")
}
fn element(out: &mut Vec<u8>, id: u32, data: &[u8]) {
    let bytes = id.to_be_bytes();
    let start = bytes.iter().position(|b| *b != 0).unwrap();
    out.extend_from_slice(&bytes[start..]);
    out.extend(size(data.len()));
    out.extend_from_slice(data);
}
fn master(id: u32, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    element(&mut out, id, data);
    out
}
fn uint(out: &mut Vec<u8>, id: u32, value: u64) {
    let bytes = value.to_be_bytes();
    let start = bytes.iter().position(|b| *b != 0).unwrap_or(7);
    element(out, id, &bytes[start..]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    fn first() -> EncodedFrame {
        let mut bytes = vec![0, 0, 0, 1];
        bytes.extend_from_slice(&[
            0x67, 0x42, 0xc0, 0x0a, 0x96, 0x54, 0x21, 0x34, 0x20, 0, 0, 3, 0, 0x20, 0, 0, 7, 0x91,
            0xe1, 0x10, 0x8d, 0x40,
        ]);
        bytes.extend_from_slice(&[0, 0, 0, 1, 0x68, 0xce, 0, 0, 0, 1, 0x65, 1]);
        EncodedFrame::h264(true, 0, bytes).unwrap()
    }

    fn limits() -> RecordingLimits {
        RecordingLimits {
            max_bytes: 1024 * 1024,
            max_duration_ms: 1000,
        }
    }
    #[test]
    fn timeline_format_and_finalize_preserve_accepted_frames() {
        let mut out =
            MatroskaRecorder::start(Cursor::new(Vec::new()), 64, 64, &first(), limits()).unwrap();
        out.push_video(
            &EncodedFrame::h264(false, 500_000, vec![0, 0, 0, 1, 0x41, 1]).unwrap(),
            500,
        )
        .unwrap();
        assert!(out.push_video(&first(), 499).is_err());
        let mut changed = first();
        changed.payload[7] = 0x1f;
        assert!(matches!(
            out.push_video(&changed, 600),
            Err(RecordingError::FormatChanged)
        ));
        assert!(out.push_video(&first(), 1001).is_err());
        assert_eq!(out.frames(), 2);
        let position = out.duration_position as usize;
        let bytes = out.finish().unwrap().into_inner();
        assert_eq!(
            f64::from_be_bytes(bytes[position..position + 8].try_into().unwrap()),
            500.0
        );
        assert!(bytes.windows(4).any(|b| b == [0x1c, 0x53, 0xbb, 0x6b]));
    }
    #[test]
    fn truncation_recovery_preserves_exact_complete_cluster_prefix() {
        let mut out =
            MatroskaRecorder::start(Cursor::new(Vec::new()), 64, 64, &first(), limits()).unwrap();
        let first_end = out.bytes();
        out.push_video(&first(), 500).unwrap();
        let second_end = out.bytes();
        let bytes = out.finish().unwrap().into_inner();
        for length in first_end..second_end {
            let recovered =
                crate::scan_recording_prefix(&mut Cursor::new(&bytes[..length as usize])).unwrap();
            assert_eq!(recovered.complete_bytes, first_end);
            assert_eq!(recovered.clusters, 1);
        }
        let recovered = crate::scan_recording_prefix(&mut Cursor::new(&bytes)).unwrap();
        assert_eq!(recovered.complete_bytes, second_end);
        assert_eq!(recovered.clusters, 2);
        assert_eq!(recovered.last_timestamp_ms, 500);
        assert!(crate::scan_recording_prefix(&mut Cursor::new(b"not a recording")).is_err());
    }
    struct Full {
        inner: Cursor<Vec<u8>>,
        allowance: usize,
    }
    impl Write for Full {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.allowance == 0 {
                return Err(io::Error::from(io::ErrorKind::StorageFull));
            }
            let size = bytes.len().min(self.allowance);
            self.allowance -= size;
            self.inner.write(&bytes[..size])
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Seek for Full {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.inner.seek(pos)
        }
    }
    #[test]
    fn disk_full_poisoning_prevents_false_success_and_further_writes() {
        let mut out = MatroskaRecorder::start(
            Full {
                inner: Cursor::new(Vec::new()),
                allowance: 4096,
            },
            64,
            64,
            &first(),
            limits(),
        )
        .unwrap();
        out.writer.allowance = 5;
        assert!(matches!(
            out.push_video(&first(), 500),
            Err(RecordingError::Storage(_))
        ));
        let length = out.writer.inner.get_ref().len();
        assert!(out.push_video(&first(), 600).is_err());
        assert_eq!(out.writer.inner.get_ref().len(), length);
        assert!(out.finish().is_err());
    }
}
