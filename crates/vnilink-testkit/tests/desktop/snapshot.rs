//! The screenshot service: a still image of the host's screen, taken by
//! the host's snapshot factory or from a picture its desktop just captured,
//! for a viewer the host lets see it, without opening a desktop.

use expect_test::expect;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use vnilink::snapshot::{
    encode_bgra, jpeg_size, HostSnapshotFactory, ScreenAccess, ScreenImage, ScreenSnapshot,
    SnapshotFailure, MAX_SNAPSHOT_BYTES, SNAPSHOT_MIN_INTERVAL,
};
use vnilink::{Core, Error, Grants, ServiceKind, ServicePhase, SessionInfo};
use vnilink_testkit::session::service_ended;
use vnilink_testkit::{traced, wait_for, Pair, TestResult, DEFAULT_WAIT};

/// An image `width` by `height` of one colour, as BGRA rows `stride` bytes
/// apart, the padding after each row a different colour.
fn solid((blue, green, red): (u8, u8, u8), width: u16, height: u16, stride: usize) -> Vec<u8> {
    let mut pixels = vec![0x11u8; stride * usize::from(height)];
    for y in 0..usize::from(height) {
        for x in 0..usize::from(width) {
            let index = y * stride + x * 4;
            pixels[index..index + 4].copy_from_slice(&[blue, green, red, 255]);
        }
    }
    pixels
}

const RED: (u8, u8, u8) = (0, 0, 255);
const GREEN: (u8, u8, u8) = (0, 255, 0);

/// The image a screen of 8 by 8 red pixels makes, named `display`.
fn red_image(captured_unix_ms: u64, display: &str) -> Result<ScreenImage, SnapshotFailure> {
    encode_bgra(8, 8, 32, &solid(RED, 8, 8, 32), captured_unix_ms, display)
}

/// A screen that is red, captured at once.
struct Picture;

impl Picture {
    fn image() -> Result<ScreenImage, SnapshotFailure> {
        red_image(1_735_000_000_000, "display-7")
    }
}

impl HostSnapshotFactory for Picture {
    fn capture(
        &self,
        deadline: Instant,
        cancel: &Arc<AtomicBool>,
        _prompt: bool,
    ) -> Result<ScreenImage, SnapshotFailure> {
        if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Err(SnapshotFailure::Cancelled);
        }
        Self::image()
    }
}

/// A capture that hands over bytes that are not the image it declares.
struct Junk;

impl HostSnapshotFactory for Junk {
    fn capture(
        &self,
        _: Instant,
        _: &Arc<AtomicBool>,
        _: bool,
    ) -> Result<ScreenImage, SnapshotFailure> {
        Ok(ScreenImage {
            jpeg: vec![1, 2, 3, 4],
            width: 1,
            height: 1,
            captured_unix_ms: 1,
            display_id: "nope".into(),
        })
    }
}

/// A capture that takes until it is cancelled or its deadline passes, and
/// says when it began.
struct Hold {
    entered: Arc<AtomicBool>,
}

impl HostSnapshotFactory for Hold {
    fn capture(
        &self,
        deadline: Instant,
        cancel: &Arc<AtomicBool>,
        _prompt: bool,
    ) -> Result<ScreenImage, SnapshotFailure> {
        self.entered.store(true, Ordering::Release);
        // A capture in progress, as a system's screenshot prompt is.
        while !cancel.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        if cancel.load(Ordering::Acquire) {
            return Err(SnapshotFailure::Cancelled);
        }
        Err(SnapshotFailure::TimedOut)
    }
}

/// A red screen that counts its captures, and names its image by whether
/// the user asked for it.
struct Count {
    hits: Arc<AtomicUsize>,
}

impl HostSnapshotFactory for Count {
    fn capture(
        &self,
        _deadline: Instant,
        cancel: &Arc<AtomicBool>,
        prompt: bool,
    ) -> Result<ScreenImage, SnapshotFailure> {
        self.hits.fetch_add(1, Ordering::SeqCst);
        if cancel.load(Ordering::Acquire) {
            return Err(SnapshotFailure::Cancelled);
        }
        red_image(4, if prompt { "prompted" } else { "counted" })
    }
}

/// A host that may be seen and controlled, whose snapshots `factory`
/// takes, and the viewer's services connection to it.
fn connected(factory: Arc<dyn HostSnapshotFactory>) -> Result<(Pair, SessionInfo), Error> {
    let pair = Pair::for_desktop();
    pair.host.core.set_snapshot_factory(Some(factory))?;
    let connection = pair.connect_services();
    Ok((pair, connection))
}

/// The viewer asks for a snapshot on `connection`, the user having asked
/// for a new capture when `prompt`.
fn request(
    controller: &Core,
    connection: &SessionInfo,
    prompt: bool,
) -> Result<ScreenSnapshot, Error> {
    controller.request_screen_snapshot(
        connection.id,
        connection.generation,
        &Arc::new(AtomicBool::new(false)),
        prompt,
    )
}

/// Seeing the host is enough for a screenshot: no control, no desktop
/// opened, and the connection's other services untouched. The image is the
/// one the host captured, and a second request too soon after is refused.
#[test]
fn view_allows_a_screenshot_without_control_audio_or_a_desktop_session() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    host.set_desktop_sharing(Grants {
        view: true,
        control: false,
        clipboard: false,
        transfer: false,
    })?;
    host.set_snapshot_factory(Some(Arc::new(Picture)))?;
    let connection = pair.connect_services();
    let files = pair.attach(&connection, ServiceKind::Files);
    let ScreenSnapshot::Ready(image) = request(controller, &connection, false)? else {
        return Err("a viewer that may see the host gets an image".into());
    };
    assert_eq!(
        (
            image.width,
            image.height,
            image.display_id.as_str(),
            image.captured_unix_ms
        ),
        (8, 8, "display-7", 1_735_000_000_000),
        "the image says what the host captured, of which display and when"
    );
    assert!(
        image.jpeg.starts_with(&[0xff, 0xd8]) && image.jpeg.len() <= MAX_SNAPSHOT_BYTES,
        "the image is a JPEG within the size bound"
    );
    assert_eq!(
        jpeg_size(&image.jpeg),
        Some((8, 8)),
        "the JPEG is the size it declares"
    );
    assert_eq!(
        image.jpeg,
        Picture::image()
            .map_err(|failure| format!("the screen's image: {failure:?}"))?
            .jpeg,
        "the viewer gets the host's image byte for byte"
    );
    assert_eq!(
        controller.controller_service(files.id)?.phase,
        ServicePhase::Active,
        "a screenshot leaves the connection's other services"
    );
    assert!(
        controller
            .controller_services()?
            .iter()
            .all(|service| service.kind != ServiceKind::Desktop),
        "a screenshot opens no desktop on the viewer"
    );
    assert!(
        host.hosted_services()?
            .iter()
            .all(|service| service.kind != ServiceKind::Desktop || service_ended(&service.phase)),
        "a screenshot opens no desktop on the host"
    );
    let again = request(controller, &connection, false);
    assert!(
        matches!(again, Err(Error::Busy)),
        "a second request too soon is refused: {again:?}"
    );
    expect![[r#""#]].assert_eq(&trace.story(&["snapshot"]));
    Ok(())
}

/// No pixels leave the host while it does not share Desktop, for a stale or
/// unknown connection, or without a snapshot factory.
#[test]
fn disabled_view_stale_identity_and_a_missing_factory_do_not_return_pixels() -> TestResult {
    let trace = traced();
    let (pair, connection) = connected(Arc::new(Picture))?;
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    host.set_service_sharing(ServiceKind::Desktop, false)?;
    assert!(
        matches!(
            request(controller, &connection, false),
            Err(Error::Unauthorized)
        ),
        "a host that does not share Desktop gives no screenshot"
    );
    host.set_service_sharing(ServiceKind::Desktop, true)?;
    let ask = |id: u64, generation: u64| {
        controller.request_screen_snapshot(id, generation, &Arc::new(AtomicBool::new(false)), false)
    };
    assert!(
        matches!(
            ask(connection.id, connection.generation.wrapping_add(1)),
            Err(Error::StaleSession)
        ),
        "a stale generation gets no screenshot"
    );
    assert!(
        matches!(
            ask(connection.id.wrapping_add(9), connection.generation),
            Err(Error::NoSession)
        ),
        "an unknown connection gets no screenshot"
    );
    host.set_snapshot_factory(None)?;
    assert!(
        matches!(
            request(controller, &connection, false),
            Err(Error::Unsupported(_))
        ),
        "a host without a snapshot factory gives no screenshot"
    );
    expect![[r#""#]].assert_eq(&trace.story(&["snapshot"]));
    Ok(())
}

/// Desktop withdrawn while the host captures: the capture is not delivered,
/// and the connection keeps its generation.
#[test]
fn a_revoked_view_and_a_bad_image_are_not_delivered() -> TestResult {
    let trace = traced();
    let entered = Arc::new(AtomicBool::new(false));
    let (pair, connection) = connected(Arc::new(Hold {
        entered: Arc::clone(&entered),
    }))?;
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let (withdrawn, captured) = std::thread::scope(|scope| {
        let worker = scope.spawn(|| request(controller, &connection, false));
        wait_for("the host to begin capturing", DEFAULT_WAIT, || {
            entered.load(Ordering::Acquire).then_some(())
        });
        let withdrawn = host.set_service_sharing(ServiceKind::Desktop, false);
        (withdrawn, worker.join())
    });
    withdrawn?;
    let captured = captured.map_err(|_| "the request did not panic")?;
    assert!(
        !matches!(captured, Ok(ScreenSnapshot::Ready(_))),
        "a capture whose view was withdrawn is not delivered: {captured:?}"
    );
    assert_eq!(
        controller.controller_session(connection.id)?.generation,
        connection.generation,
        "withdrawing the view does not reconnect"
    );
    expect![[r#""#]].assert_eq(&trace.story(&["snapshot"]));
    Ok(())
}

/// An image that is not what it declares reaches the viewer as unavailable,
/// not as pixels.
#[test]
fn an_unacceptable_image_is_reported_unavailable() -> TestResult {
    let trace = traced();
    let (pair, connection) = connected(Arc::new(Junk))?;
    let answer = request(&pair.controller.core, &connection, false);
    assert!(
        matches!(answer, Ok(ScreenSnapshot::Unavailable)),
        "an unacceptable image is reported unavailable: {answer:?}"
    );
    expect![[r#""#]].assert_eq(&trace.story(&["snapshot"]));
    Ok(())
}

/// A picture the host's desktop just captured is offered once and answers
/// the next screenshot without a capture; a screenshot the user asks for
/// captures anew.
#[test]
fn a_fresh_desktop_frame_is_reused_until_the_user_asks_for_a_new_capture() -> TestResult {
    let trace = traced();
    let hits = Arc::new(AtomicUsize::new(0));
    let (pair, connection) = connected(Arc::new(Count {
        hits: Arc::clone(&hits),
    }))?;
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    // A capture asks before reading its pixels: wanted until one is taken.
    let now = Instant::now();
    assert!(
        host.wants_attested_picture(now),
        "the host wants a picture before it has one"
    );
    host.offer_attested_picture(now, 8, 8, 32, &solid(GREEN, 8, 8, 32), "from-desktop");
    assert!(
        !host.wants_attested_picture(Instant::now()),
        "the host wants no picture once it has a fresh one"
    );
    let ScreenSnapshot::Ready(image) = request(controller, &connection, false)? else {
        return Err("the fresh desktop picture answers the screenshot".into());
    };
    let answered = Instant::now();
    assert_eq!(
        image.display_id, "from-desktop",
        "the screenshot is the desktop's picture"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "a fresh desktop picture needs no capture"
    );
    // Screenshots are spaced: the next is admitted once the interval passed.
    wait_for(
        "the screenshot interval to pass",
        SNAPSHOT_MIN_INTERVAL + DEFAULT_WAIT,
        || (answered.elapsed() >= SNAPSHOT_MIN_INTERVAL).then_some(()),
    );
    let ScreenSnapshot::Ready(image) = request(controller, &connection, true)? else {
        return Err("a screenshot the user asked for is captured".into());
    };
    assert_eq!(
        image.display_id, "prompted",
        "a screenshot the user asked for is captured anew"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "the user's screenshot is exactly one capture"
    );
    expect![[r#""#]].assert_eq(&trace.story(&["snapshot"]));
    Ok(())
}

/// Asking whether a screenshot is allowed answers from what the host shares
/// and captures nothing.
#[test]
fn a_view_probe_does_not_capture() -> TestResult {
    let trace = traced();
    let hits = Arc::new(AtomicUsize::new(0));
    let (pair, connection) = connected(Arc::new(Count {
        hits: Arc::clone(&hits),
    }))?;
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    assert_eq!(
        controller.screen_capture_access(connection.id, connection.generation)?,
        ScreenAccess::Allowed,
        "a host that shares Desktop allows a screenshot"
    );
    host.set_service_sharing(ServiceKind::Desktop, false)?;
    assert_eq!(
        controller.screen_capture_access(connection.id, connection.generation)?,
        ScreenAccess::Denied,
        "a host that does not share Desktop denies one"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "asking whether a screenshot is allowed captures nothing"
    );
    expect![[r#""#]].assert_eq(&trace.story(&["snapshot"]));
    Ok(())
}

/// The viewer cancelling a capture in progress gets a cancelled answer, not
/// a missing connection.
#[test]
fn cancelling_a_running_capture_is_not_a_missing_session() -> TestResult {
    let trace = traced();
    let entered = Arc::new(AtomicBool::new(false));
    let (pair, connection) = connected(Arc::new(Hold {
        entered: Arc::clone(&entered),
    }))?;
    let controller = &pair.controller.core;
    let cancel = Arc::new(AtomicBool::new(false));
    let captured = std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            controller.request_screen_snapshot(connection.id, connection.generation, &cancel, false)
        });
        wait_for("the host to begin capturing", DEFAULT_WAIT, || {
            entered.load(Ordering::Acquire).then_some(())
        });
        cancel.store(true, Ordering::Release);
        worker.join().map_err(|_| "the request did not panic")
    })?;
    assert!(
        matches!(captured, Ok(ScreenSnapshot::Cancelled)),
        "a cancelled capture answers cancelled: {captured:?}"
    );
    expect![[r#""#]].assert_eq(&trace.story(&["snapshot"]));
    Ok(())
}
