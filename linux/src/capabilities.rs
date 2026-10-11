//! Capability state for the UI. Turns `Capabilities` into explicit cards
//! instead of a black screen or a silent dead end.

use vnilink::Capabilities;

/// Video state of this installation, from its own capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoStatus {
    /// Everything needed is present.
    Ready,
    /// A capture adapter is installed but no H.264 encoder was reported.
    /// The host admits no session in this state.
    NoHostEncoder,
    /// This computer cannot decode video, so connecting is pointless.
    NoLocalDecoder,
}

pub fn video_status(capabilities: &Capabilities) -> VideoStatus {
    if capabilities.desktop_capture && !capabilities.h264_encode {
        VideoStatus::NoHostEncoder
    } else if !capabilities.h264_decode {
        VideoStatus::NoLocalDecoder
    } else {
        VideoStatus::Ready
    }
}

pub fn host_encoder_sentence(status: VideoStatus) -> Option<&'static str> {
    match status {
        VideoStatus::NoHostEncoder => Some("Video unavailable: no H.264 encoder on this computer."),
        _ => None,
    }
}

pub fn local_decoder_sentence(status: VideoStatus) -> Option<&'static str> {
    match status {
        VideoStatus::NoLocalDecoder => Some("This computer cannot decode video: no H.264 decoder."),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vnilink::Capabilities;

    fn capable() -> Capabilities {
        Capabilities {
            desktop_capture: true,
            keyboard: true,
            pointer: true,
            clipboard: false,
            file_transfer: false,
            audio: false,
            monitors: 1,
            h264_encode: true,
            h264_decode: true,
        }
    }

    #[test]
    fn states_cover_host_encode_and_local_decode() {
        assert_eq!(video_status(&capable()), VideoStatus::Ready);
        let mut no_encoder = capable();
        no_encoder.h264_encode = false;
        assert_eq!(video_status(&no_encoder), VideoStatus::NoHostEncoder);
        // Without a capture adapter there is nothing to encode for, so only
        // the missing local decoder is reported.
        let mut bare = Capabilities::UNSUPPORTED;
        bare.h264_decode = false;
        assert_eq!(video_status(&bare), VideoStatus::NoLocalDecoder);
        assert!(host_encoder_sentence(VideoStatus::NoHostEncoder)
            .unwrap()
            .contains("no H.264 encoder"));
        assert!(local_decoder_sentence(VideoStatus::NoLocalDecoder)
            .unwrap()
            .contains("no H.264 decoder"));
        assert_eq!(host_encoder_sentence(VideoStatus::Ready), None);
    }
}
