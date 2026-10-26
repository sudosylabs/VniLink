//! Still pictures: an AV1 4:4:4 picture of a still screen, carried outside
//! the stream's codec only where both sides agreed to it.

use expect_test::expect;

use vnilink::{
    EncodedFrame, Error, FrameRole, HostCapture, RemoteDesktopService, ServiceInfo, ServiceKind,
    VideoCodec,
};
use vnilink_testkit::desktop::next_picture;
use vnilink_testkit::desktop::window_decoders;
use vnilink_testkit::{traced, Pair, TestResult};

use super::streams::SCREEN;

/// A host's capture of one display that sends still pictures, or not.
struct Capture {
    stills: bool,
}

impl HostCapture for Capture {
    fn monitor(&self) -> (u16, u16) {
        SCREEN
    }

    fn stills(&self) -> bool {
        self.stills
    }
}

/// A desktop open between a host whose capture sends stills when
/// `host_sends` and a viewer whose window shows them when `viewer_shows`,
/// with the host's side of it.
struct Opened {
    pair: Pair,
    desktop: RemoteDesktopService,
    remote: ServiceInfo,
}

impl Opened {
    fn new(host_sends: bool, viewer_shows: bool) -> Result<Self, Error> {
        let pair = Pair::for_desktop();
        pair.host
            .core
            .set_capture(Box::new(Capture { stills: host_sends }))?;
        pair.controller
            .core
            .set_window_decoders(window_decoders(viewer_shows))?;
        let connection = pair.connect_services();
        let desktop = pair.controller.open_desktop(&connection)?;
        let remote = pair
            .host
            .hosted_for(ServiceKind::Desktop, &pair.controller)
            .ok_or(Error::NoSession)?;
        Ok(Self {
            pair,
            desktop,
            remote,
        })
    }

    fn publish(&self, frame: EncodedFrame) -> Result<(), Error> {
        self.pair
            .host
            .core
            .publish_desktop_video(self.remote.id, self.remote.generation, frame)
    }

    /// Closes the desktop, once the host has released it.
    fn close(self) {
        self.desktop.close();
        self.pair.host.wait_released(Some(ServiceKind::Desktop));
    }
}

/// Stills are agreed only where the host's capture sends them and the
/// viewer's window shows them, and both sides say the same.
#[test]
fn stills_are_agreed_only_when_the_window_shows_them_and_the_host_sends_them() -> TestResult {
    let trace = traced();
    for (sends, shows) in [(true, true), (true, false), (false, true), (false, false)] {
        let opened = Opened::new(sends, shows)?;
        let agreed = sends && shows;
        assert_eq!(
            opened.desktop.desktop_info()?.stills,
            agreed,
            "the viewer agrees stills only when the host sends them ({sends}) and its window shows them ({shows})"
        );
        assert_eq!(
            opened
                .pair
                .host
                .core
                .hosted_desktop_info(opened.remote.id, opened.remote.generation)?
                .stills,
            agreed,
            "the host agrees stills only when it sends them ({sends}) and the window shows them ({shows})"
        );
        opened.close();
    }
    trace.wait_for_records("service phase", 8);
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
    "#]].assert_eq(&trace.story(&["still", "service"]));
    Ok(())
}

/// An agreed still reaches the viewer as an AV1 picture between the
/// stream's own, whether the stream is a session's or forced to a service's
/// own stream; the stream goes on after it, and the host counts it as a
/// still, not a keyframe.
#[test]
fn an_agreed_still_reaches_the_viewer_outside_the_streams_codec() -> TestResult {
    let trace = traced();
    for stream in [false, true] {
        let opened = Opened::new(true, true)?;
        if stream {
            opened
                .pair
                .host
                .core
                .force_desktop_stream(opened.remote.id, opened.remote.generation)?;
        }
        let video = opened.desktop.subscribe_video()?;
        opened.publish(EncodedFrame::h264(true, 1, vec![7; 2_000])?)?;
        assert_eq!(
            next_picture(&video, "the first picture").role,
            FrameRole::Changed,
            "the first picture is a changed one"
        );
        opened.publish(EncodedFrame::still(2, vec![9; 20_000])?)?;
        let still = next_picture(&video, "the still");
        assert_eq!(
            (still.role, still.codec),
            (FrameRole::Still, VideoCodec::Av1),
            "the still arrives as an AV1 still"
        );
        assert_eq!(still.payload, vec![9; 20_000], "the still arrives whole");
        opened.publish(EncodedFrame::h264(false, 3, vec![8; 500])?.unchanged())?;
        assert_eq!(
            next_picture(&video, "the picture after the still").role,
            FrameRole::Unchanged,
            "the stream goes on after the still"
        );
        let pipeline = opened
            .pair
            .host
            .core
            .diagnostic_snapshot()?
            .pipelines
            .into_iter()
            .find(|pipeline| pipeline.hosted)
            .ok_or("the host's pipeline")?;
        assert_eq!(
            (pipeline.stills_sent, pipeline.still_bytes),
            (1, 20_000),
            "the host counts one still of its size"
        );
        assert_eq!(
            pipeline.keyframes_sent, 1,
            "a still is not the stream's keyframe"
        );
        opened.close();
    }
    trace.wait_for_records("service phase", 4);
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
    "#]].assert_eq(&trace.story(&["still", "service"]));
    Ok(())
}

/// A host cannot send a still where the viewer's window does not show them.
#[test]
fn a_still_is_refused_where_it_was_not_agreed() -> TestResult {
    let trace = traced();
    let opened = Opened::new(true, false)?;
    assert!(
        matches!(
            opened.publish(EncodedFrame::still(1, vec![1; 100])?),
            Err(Error::Unsupported(_))
        ),
        "a still that was not agreed is refused"
    );
    opened.close();
    trace.wait_for_records("service phase", 2);
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
    "#]].assert_eq(&trace.story(&["still", "service"]));
    Ok(())
}
