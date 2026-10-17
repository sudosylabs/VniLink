use super::support::{pair_for_control, start_direct};
use crate::core::snapshot::snapshot_admitted;
use crate::snapshot::{
    classify_stop, encode_bgra, fitted_size, jpeg_size, tap_is_fresh, validate_image,
    HostSnapshotFactory, ScreenAccess, ScreenImage, ScreenSnapshot, SnapshotFailure,
    MAX_SNAPSHOT_BYTES, MAX_SNAPSHOT_HEIGHT, MAX_SNAPSHOT_JOBS, MAX_SNAPSHOT_WIDTH,
    SNAPSHOT_MIN_INTERVAL,
};
use crate::{Error, Grants, ServiceKind, ServicePhase, SessionPhase};
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn wait(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < until, "timed out waiting for a session");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn connect(controller: &crate::Core, host: &crate::Core) -> crate::SessionInfo {
    let session = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    wait(|| controller.controller_session(session.id).unwrap().phase == SessionPhase::Active);
    controller.controller_session(session.id).unwrap()
}

fn solid(blue: u8, green: u8, red: u8, width: u16, height: u16, stride: usize) -> Vec<u8> {
    let mut pixels = vec![0x11u8; stride * usize::from(height)];
    for y in 0..usize::from(height) {
        for x in 0..usize::from(width) {
            let index = y * stride + x * 4;
            pixels[index] = blue;
            pixels[index + 1] = green;
            pixels[index + 2] = red;
            pixels[index + 3] = 255;
        }
    }
    pixels
}

fn decoded(jpeg: &[u8]) -> (u16, u16, Vec<u8>) {
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(jpeg));
    let pixels = decoder.decode().expect("jpeg");
    let info = decoder.info().expect("jpeg info");
    (info.width, info.height, pixels)
}

fn near(actual: u8, expected: u8) -> bool {
    actual.abs_diff(expected) <= 12
}

#[test]
fn fitted_sizes_keep_aspect_and_never_enlarge() {
    assert_eq!(fitted_size(2, 2), Some((2, 2)));
    assert_eq!(fitted_size(1920, 1080), Some((640, 360)));
    assert_eq!(fitted_size(1080, 1920), Some((202, 360)));
    assert_eq!(fitted_size(3440, 1440), Some((640, 267)));
    assert_eq!(fitted_size(0, 10), None);
    let (width, height) = fitted_size(8000, 1000).unwrap();
    assert!(width <= MAX_SNAPSHOT_WIDTH && height <= MAX_SNAPSHOT_HEIGHT);
    assert!(width < 8000 && height < 1000);
}

#[test]
fn encode_reads_strided_bgra_and_stays_inside_the_byte_cap() {
    let red = solid(0, 0, 255, 8, 8, 8 * 4 + 16);
    let image = encode_bgra(8, 8, 8 * 4 + 16, &red, 1_700_000_000_000, "panel").unwrap();
    assert!(image.jpeg.len() <= MAX_SNAPSHOT_BYTES);
    assert_eq!((image.width, image.height), (8, 8));
    let (width, height, pixels) = decoded(&image.jpeg);
    assert_eq!((width, height), (8, 8));
    assert!(pixels
        .chunks_exact(3)
        .all(|pixel| near(pixel[0], 255) && near(pixel[1], 0) && near(pixel[2], 0)));

    let green = solid(0, 255, 0, 8, 8, 32);
    let image = encode_bgra(8, 8, 32, &green, 5, "g").unwrap();
    let (_, _, pixels) = decoded(&image.jpeg);
    assert!(pixels
        .chunks_exact(3)
        .all(|pixel| near(pixel[0], 0) && near(pixel[1], 255) && near(pixel[2], 0)));

    let blue = solid(255, 0, 0, 8, 8, 32);
    let image = encode_bgra(8, 8, 32, &blue, 5, "b").unwrap();
    let (_, _, pixels) = decoded(&image.jpeg);
    assert!(pixels
        .chunks_exact(3)
        .all(|pixel| near(pixel[0], 0) && near(pixel[1], 0) && near(pixel[2], 255)));

    let wide = solid(0, 0, 255, 1920, 1080, 1920 * 4);
    let image = encode_bgra(1920, 1080, 1920 * 4, &wide, 9, "wide").unwrap();
    assert_eq!((image.width, image.height), (640, 360));
    assert!(image.jpeg.len() <= MAX_SNAPSHOT_BYTES);
    assert!(encode_bgra(1, 1, 2, &[0, 0, 0, 0], 0, "short").is_err());
    assert!(encode_bgra(1, 1, 4, &[0, 0, 0, 0], 0, &"x".repeat(65)).is_err());
}

#[test]
fn admission_bounds_jobs_and_spacing() {
    assert!(snapshot_admitted(0, None));
    assert!(snapshot_admitted(
        MAX_SNAPSHOT_JOBS - 1,
        Some(SNAPSHOT_MIN_INTERVAL)
    ));
    assert!(!snapshot_admitted(MAX_SNAPSHOT_JOBS, None));
    assert!(!snapshot_admitted(
        0,
        Some(SNAPSHOT_MIN_INTERVAL - Duration::from_millis(1))
    ));
}

struct Picture;

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
        encode_bgra(
            8,
            8,
            32,
            &solid(0, 0, 255, 8, 8, 32),
            1_735_000_000_000,
            "display-7",
        )
    }
}

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
        while !cancel.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if cancel.load(Ordering::Acquire) {
            return Err(SnapshotFailure::Cancelled);
        }
        Err(SnapshotFailure::TimedOut)
    }
}

fn request(
    controller: &crate::Core,
    session: &crate::SessionInfo,
) -> Result<ScreenSnapshot, Error> {
    request_with(controller, session, false)
}

fn request_with(
    controller: &crate::Core,
    session: &crate::SessionInfo,
    prompt: bool,
) -> Result<ScreenSnapshot, Error> {
    controller.request_screen_snapshot(
        session.id,
        session.generation,
        &Arc::new(AtomicBool::new(false)),
        prompt,
    )
}

#[test]
fn view_allows_a_screenshot_without_control_audio_or_a_desktop_session() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    host.set_desktop_sharing(Grants {
        view: true,
        control: false,
        clipboard: false,
        transfer: false,
    })
    .unwrap();
    host.set_snapshot_factory(Some(Arc::new(Picture))).unwrap();
    let session = connect(&controller, &host);
    let files = controller
        .attach_controller_service(session.id, session.generation, ServiceKind::Files)
        .unwrap();
    let shot = request(&controller, &session).unwrap();
    let ScreenSnapshot::Ready(image) = shot else {
        panic!("expected an image");
    };
    assert_eq!(
        (
            image.width,
            image.height,
            image.display_id.as_str(),
            image.captured_unix_ms
        ),
        (8, 8, "display-7", 1_735_000_000_000)
    );
    assert!(image.jpeg.starts_with(&[0xff, 0xd8]) && image.jpeg.len() <= MAX_SNAPSHOT_BYTES);
    let (_, _, pixels) = decoded(&image.jpeg);
    assert!(pixels
        .chunks_exact(3)
        .all(|pixel| near(pixel[0], 255) && near(pixel[1], 0) && near(pixel[2], 0)));
    assert_eq!(
        controller.controller_service(files.id).unwrap().phase,
        ServicePhase::Active
    );
    assert!(controller
        .controller_services()
        .unwrap()
        .iter()
        .all(|service| service.kind != ServiceKind::Desktop));
    assert!(host
        .hosted_services()
        .unwrap()
        .iter()
        .all(|service| service.kind != ServiceKind::Desktop || service.phase.finished()));
    let again = request(&controller, &session);
    assert!(matches!(again, Err(Error::Busy)), "{again:?}");
}

#[test]
fn disabled_view_stale_identity_and_a_missing_factory_do_not_return_pixels() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    host.set_snapshot_factory(Some(Arc::new(Picture))).unwrap();
    let session = connect(&controller, &host);
    host.set_service_sharing(ServiceKind::Desktop, false)
        .unwrap();
    assert!(matches!(
        request(&controller, &session),
        Err(Error::Unauthorized)
    ));
    host.set_service_sharing(ServiceKind::Desktop, true)
        .unwrap();
    assert!(matches!(
        controller.request_screen_snapshot(
            session.id,
            session.generation.wrapping_add(1),
            &Arc::new(AtomicBool::new(false)),
            false,
        ),
        Err(Error::StaleSession)
    ));
    assert!(matches!(
        controller.request_screen_snapshot(
            session.id.wrapping_add(9),
            session.generation,
            &Arc::new(AtomicBool::new(false)),
            false,
        ),
        Err(Error::NoSession)
    ));
    host.set_snapshot_factory(None).unwrap();
    assert!(matches!(
        request(&controller, &session),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn a_revoked_view_and_a_bad_image_are_not_delivered() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    let entered = Arc::new(AtomicBool::new(false));
    host.set_snapshot_factory(Some(Arc::new(Hold {
        entered: Arc::clone(&entered),
    })))
    .unwrap();
    let session = connect(&controller, &host);
    let session_id = session.id;
    let generation = session.generation;
    let captured = std::thread::scope(|scope| {
        let worker = scope.spawn(|| request(&controller, &session));
        wait(|| entered.load(Ordering::Acquire));
        host.set_service_sharing(ServiceKind::Desktop, false)
            .unwrap();
        worker.join().expect("snapshot thread")
    });
    assert!(
        !matches!(captured, Ok(ScreenSnapshot::Ready(_))),
        "{captured:?}"
    );
    assert_eq!(
        controller
            .controller_session(session_id)
            .unwrap()
            .generation,
        generation
    );
}

#[test]
fn an_unacceptable_image_is_reported_unavailable() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    host.set_snapshot_factory(Some(Arc::new(Junk))).unwrap();
    let session = connect(&controller, &host);
    assert!(matches!(
        request(&controller, &session),
        Ok(ScreenSnapshot::Unavailable)
    ));
}

#[test]
fn jpeg_dimensions_must_match_the_declared_image() {
    let image = encode_bgra(8, 8, 32, &solid(0, 0, 255, 8, 8, 32), 9, "panel").unwrap();
    assert_eq!(jpeg_size(&image.jpeg), Some((image.width, image.height)));
    assert!(validate_image(&image).is_ok());
    let mut wrong = ScreenImage {
        jpeg: image.jpeg.clone(),
        width: 9,
        height: image.height,
        captured_unix_ms: image.captured_unix_ms,
        display_id: image.display_id.clone(),
    };
    assert!(validate_image(&wrong).is_err());
    wrong.width = image.width;
    wrong.jpeg[0] = 0;
    assert!(validate_image(&wrong).is_err());
}

#[test]
fn classify_stop_lets_cancel_win_and_a_tap_expires_after_two_seconds() {
    let cancel = AtomicBool::new(true);
    let deadline = Instant::now() + Duration::from_secs(5);
    assert_eq!(classify_stop(&cancel, deadline), SnapshotFailure::Cancelled);
    cancel.store(false, Ordering::Release);
    assert_eq!(
        classify_stop(&cancel, Instant::now() - Duration::from_millis(1)),
        SnapshotFailure::TimedOut
    );
    assert_eq!(
        classify_stop(&cancel, deadline),
        SnapshotFailure::Unavailable
    );
    let now = Instant::now();
    assert!(tap_is_fresh(now, now + Duration::from_secs(2)));
    assert!(!tap_is_fresh(
        now,
        now + Duration::from_secs(2) + Duration::from_millis(1)
    ));
}

#[test]
fn a_cancelled_request_is_not_a_missing_session() {
    let controller = start_direct();
    let cancel = Arc::new(AtomicBool::new(true));
    assert!(matches!(
        controller.request_screen_snapshot(1, 1, &cancel, false),
        Ok(ScreenSnapshot::Cancelled)
    ));
}

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
        encode_bgra(
            8,
            8,
            32,
            &solid(0, 0, 255, 8, 8, 32),
            4,
            if prompt { "prompted" } else { "counted" },
        )
    }
}

#[test]
fn a_fresh_desktop_frame_is_reused_until_the_user_asks_for_a_new_capture() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    let hits = Arc::new(AtomicUsize::new(0));
    host.set_snapshot_factory(Some(Arc::new(Count {
        hits: Arc::clone(&hits),
    })))
    .unwrap();
    let session = connect(&controller, &host);
    host.offer_attested_picture(
        Instant::now(),
        8,
        8,
        32,
        &solid(0, 255, 0, 8, 8, 32),
        "from-desktop",
    );
    let shot = request(&controller, &session).unwrap();
    let ScreenSnapshot::Ready(image) = shot else {
        panic!("expected the attested frame");
    };
    assert_eq!(image.display_id, "from-desktop");
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    std::thread::sleep(SNAPSHOT_MIN_INTERVAL);
    let prompted = request_with(&controller, &session, true).unwrap();
    let ScreenSnapshot::Ready(image) = prompted else {
        panic!("expected a prompted capture");
    };
    assert_eq!(image.display_id, "prompted");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[test]
fn a_view_probe_does_not_capture() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    let hits = Arc::new(AtomicUsize::new(0));
    host.set_snapshot_factory(Some(Arc::new(Count {
        hits: Arc::clone(&hits),
    })))
    .unwrap();
    let session = connect(&controller, &host);
    assert_eq!(
        controller
            .screen_capture_access(session.id, session.generation)
            .unwrap(),
        ScreenAccess::Allowed
    );
    host.set_service_sharing(ServiceKind::Desktop, false)
        .unwrap();
    assert_eq!(
        controller
            .screen_capture_access(session.id, session.generation)
            .unwrap(),
        ScreenAccess::Denied
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[test]
fn cancelling_a_running_capture_is_not_a_missing_session() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    let entered = Arc::new(AtomicBool::new(false));
    host.set_snapshot_factory(Some(Arc::new(Hold {
        entered: Arc::clone(&entered),
    })))
    .unwrap();
    let session = connect(&controller, &host);
    let cancel = Arc::new(AtomicBool::new(false));
    let captured = std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            controller.request_screen_snapshot(session.id, session.generation, &cancel, false)
        });
        wait(|| entered.load(Ordering::Acquire));
        cancel.store(true, Ordering::Release);
        worker.join().expect("snapshot thread")
    });
    assert!(
        matches!(captured, Ok(ScreenSnapshot::Cancelled)),
        "{captured:?}"
    );
}
