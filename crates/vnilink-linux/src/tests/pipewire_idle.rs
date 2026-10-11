use super::*;

#[test]
fn idle_portal_keeps_the_picture_and_propagates_capture_failure() {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).unwrap();
    let shared = Shared {
        cursor: Mutex::new(None),
        size: Mutex::new(Some((2, 2))),
        channel: FrameChannel::new(),
        format: Mutex::new(Some(VideoFormat::BGRx)),
        modifier: Mutex::new(DRM_FORMAT_MOD_INVALID),
        modifier_explicit: AtomicBool::new(false),
        delivery: Mutex::new(Some(PortalDelivery {
            format: PortalFormat::Bgrx,
            buffer: PortalBuffer::Memory,
        })),
    };
    assert!(wait_frame(&mainloop, &shared, 0).unwrap().is_none());
    shared.channel.push(ChannelSlot {
        width: 2,
        height: 2,
        stride: 8,
        bytes: vec![40; 16],
    });
    let (first, _) = wait_frame(&mainloop, &shared, 0).unwrap().unwrap();
    // PipeWire can send nothing further when the monitor has no damage.
    let (idle, _) = wait_frame(&mainloop, &shared, 33_333).unwrap().unwrap();
    assert_eq!(first.row(0), idle.row(0));
    assert_eq!(idle.timestamp_us(), 33_333);
    shared.channel.fail("portal closed");
    assert!(matches!(
        wait_frame(&mainloop, &shared, 66_666),
        Err(Error::Unsupported("portal closed"))
    ));
}
