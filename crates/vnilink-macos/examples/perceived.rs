//! Drives and reads the perceived-quality page (`npm run measure:perceived`)
//! through whatever remote desktop application shows it full screen on this
//! Mac's main display: it captures the display with ScreenCaptureKit, finds
//! the page (`vnilink_media::measure::perceived`), moves and clicks the real
//! pointer with Core Graphics events, and writes every observation as one
//! JSON line to `<out>/events.jsonl`, with crops of the test card after each
//! card swap as raw BGRA files beside it. Times are ms since the start, on
//! the display's clock for pictures and this process's for input.
//!
//! The capture is its own minimal stream: its handler copies only the
//! regions asked for and lets the buffer go at once. ScreenCaptureKit often
//! stamps a picture with the refresh it is shown on, just ahead of now; the
//! stream converts those stamps too, so no picture is lost for want of a time.
//!
//! Needs Screen Recording and Accessibility for whichever application it
//! runs under.
//!
//! `cargo run -p vnilink-macos --release --example perceived -- <out dir> [seconds to wait for the page] [display width in pixels]`
//!
//! ScreenCaptureKit lists a display's size in points; the pixel width
//! (`system_profiler SPDisplaysDataType`) makes the capture full resolution,
//! as a viewer's screen shows it.

#[cfg(target_os = "macos")]
mod capture {
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use screencapturekit::cm::{
        CMSampleBuffer, CMSampleBufferExt, CMSampleBufferSCExt, SCFrameStatus,
    };
    use screencapturekit::shareable_content::SCShareableContent;
    use screencapturekit::stream::configuration::{PixelFormat, SCStreamConfiguration};
    use screencapturekit::stream::content_filter::SCContentFilter;
    use screencapturekit::stream::output_trait::SCStreamOutputTrait;
    use screencapturekit::stream::output_type::SCStreamOutputType;
    use screencapturekit::stream::SCStream;
    use vnilink_media::measure::perceived::Rect;
    use vnilink_media::RawFrame;

    /// What the next pictures should carry: the column, or the whole
    /// picture before it is found; and once, the card.
    #[derive(Clone, Copy, Default)]
    pub struct Want {
        pub column: Option<Rect>,
        pub card: Option<Rect>,
        /// While the whole picture is wanted, one picture in this many.
        pub every: u32,
    }

    /// One displayed picture, as much of it as was wanted.
    pub struct Picture {
        pub column: RawFrame,
        pub card: Option<RawFrame>,
        /// How long the handler held the buffer locked, in ms.
        pub locked_ms: f64,
    }

    /// Host time as an instant, either side of now: ScreenCaptureKit stamps
    /// a picture with the refresh it is shown on, which may be just ahead.
    /// `vnilink_macos::host_time_instant` gives nothing for those.
    #[derive(Clone, Copy)]
    pub struct Clock {
        nanos_per_tick: f64,
    }

    impl Clock {
        /// Measures the host clock's rate against `Instant` for 200 ms.
        pub fn measure() -> Self {
            let (ticks, at) = (vnilink_macos::host_time_now(), Instant::now());
            std::thread::sleep(std::time::Duration::from_millis(200));
            let (later, then) = (vnilink_macos::host_time_now(), Instant::now());
            Self {
                nanos_per_tick: then.duration_since(at).as_nanos() as f64
                    / later.saturating_sub(ticks).max(1) as f64,
            }
        }

        pub fn instant(self, host_time: u64) -> Option<Instant> {
            let (now_ticks, now) = (vnilink_macos::host_time_now(), Instant::now());
            let nanos = |ticks: u64| {
                std::time::Duration::from_nanos((ticks as f64 * self.nanos_per_tick) as u64)
            };
            if host_time >= now_ticks {
                now.checked_add(nanos(host_time - now_ticks))
            } else {
                now.checked_sub(nanos(now_ticks - host_time))
            }
        }
    }

    struct Handler {
        clock: Clock,
        want: Arc<Mutex<Want>>,
        pictures: mpsc::SyncSender<Picture>,
        seen: Mutex<u32>,
    }

    /// `rect` of a locked buffer, clamped to it, as its own picture.
    fn region(
        bytes: &[u8],
        stride: usize,
        size: (usize, usize),
        rect: Option<Rect>,
        at: Instant,
    ) -> Option<RawFrame> {
        let rect = rect.unwrap_or(Rect {
            x: 0.0,
            y: 0.0,
            width: size.0 as f64,
            height: size.1 as f64,
        });
        let left = (rect.x.max(0.0) as usize).min(size.0.saturating_sub(1));
        let top = (rect.y.max(0.0) as usize).min(size.1.saturating_sub(1));
        let right = ((rect.x + rect.width).ceil().max(0.0) as usize).min(size.0);
        let bottom = ((rect.y + rect.height).ceil().max(0.0) as usize).min(size.1);
        let (width, height) = (right.checked_sub(left)?, bottom.checked_sub(top)?);
        let mut pixels = Vec::with_capacity(width * height * 4);
        for y in top..bottom {
            let from = y * stride + left * 4;
            pixels.extend_from_slice(bytes.get(from..from + width * 4)?);
        }
        RawFrame::bgra(
            u16::try_from(width).ok()?,
            u16::try_from(height).ok()?,
            0,
            pixels,
        )
        .ok()
        .map(|frame| frame.with_captured_at(Some(at)))
    }

    impl SCStreamOutputTrait for Handler {
        fn did_output_sample_buffer(&self, sample: CMSampleBuffer, _: SCStreamOutputType) {
            if sample.frame_status() != Some(SCFrameStatus::Complete) {
                return;
            }
            let (Some(buffer), Some(at)) = (
                sample.pixel_buffer(),
                sample
                    .display_time()
                    .and_then(|time| self.clock.instant(time)),
            ) else {
                return;
            };
            let mut want = self
                .want
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if want.column.is_none() {
                let mut seen = self
                    .seen
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                *seen += 1;
                if !seen.is_multiple_of(want.every.max(1)) {
                    return;
                }
            }
            let locked = Instant::now();
            let Ok(guard) = buffer.lock_read_only() else {
                return;
            };
            // The stream has finished this buffer; the guard only reads it.
            let Some(bytes) = (unsafe { guard.as_slice() }) else {
                return;
            };
            let size = (buffer.width(), buffer.height());
            let stride = guard.bytes_per_row();
            let Some(column) = region(bytes, stride, size, want.column, at) else {
                return;
            };
            let card = want
                .card
                .take()
                .and_then(|rect| region(bytes, stride, size, Some(rect), at));
            drop(want);
            let locked_ms = locked.elapsed().as_secs_f64() * 1000.0;
            drop(guard);
            let _ = self.pictures.try_send(Picture {
                column,
                card,
                locked_ms,
            });
        }
    }

    /// A running stream of the display `display_id` at `width` by `height`
    /// pixels, 60 pictures a second at most.
    pub struct Capture {
        _stream: SCStream,
        pub want: Arc<Mutex<Want>>,
        pub pictures: mpsc::Receiver<Picture>,
    }

    pub fn start(
        display_id: u64,
        width: u32,
        height: u32,
    ) -> Result<Capture, Box<dyn std::error::Error>> {
        let content = SCShareableContent::get().map_err(|_| "Screen Recording is not granted")?;
        let displays = content.displays();
        let display = displays
            .iter()
            .find(|display| u64::from(display.display_id()) == display_id)
            .ok_or("the main display is not shareable")?;
        let filter = SCContentFilter::create()
            .with_display(display)
            .with_excluding_windows(&[])
            .build()
            .map_err(|_| "the display filter could not be built")?;
        let configuration = SCStreamConfiguration::new()
            .with_queue_depth(8)
            .with_width(width)
            .with_height(height)
            .with_pixel_format(PixelFormat::BGRA)
            .with_shows_cursor(false)
            .with_minimum_frame_interval(&apple_cf::cm::CMTime::new(1, 60))
            // The page draws sRGB; the reference card is sRGB.
            .with_color_space_name("kCGColorSpaceSRGB")
            .map_err(|_| "the capture colour space")?;
        let want = Arc::new(Mutex::new(Want {
            every: 6,
            ..Want::default()
        }));
        let (sender, pictures) = mpsc::sync_channel(8);
        let mut stream =
            SCStream::new(&filter, &configuration).map_err(|_| "ScreenCaptureKit did not start")?;
        stream
            .add_output_handler(
                Handler {
                    clock: Clock::measure(),
                    want: Arc::clone(&want),
                    pictures: sender,
                    seen: Mutex::new(0),
                },
                SCStreamOutputType::Screen,
            )
            .map_err(|_| "ScreenCaptureKit did not start")?;
        stream
            .start_capture()
            .map_err(|_| "ScreenCaptureKit did not start")?;
        Ok(Capture {
            _stream: stream,
            want,
            pictures,
        })
    }
}

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    use std::time::{Duration, Instant};

    use objc2_core_foundation::CGPoint;
    use objc2_core_graphics::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton};
    use vnilink_macos::list_displays;
    use vnilink_media::measure::perceived::{locate, read, Mapping, Reading, Role};
    use vnilink_media::RawFrame;

    let mut args = std::env::args().skip(1);
    let out = std::path::PathBuf::from(args.next().ok_or("output directory required")?);
    let wait: u64 = args.next().map_or(Ok(120), |seconds| seconds.parse())?;
    let pixel_width: Option<u32> = args.next().map(|width| width.parse()).transpose()?;
    std::fs::create_dir_all(&out)?;
    let display = list_displays(1)?
        .into_iter()
        .find(|display| display.origin_x == 0 && display.origin_y == 0)
        .ok_or("no main display")?;
    // Core Graphics events are in points; the capture is in pixels.
    let density = pixel_width.map_or(1.0, |width| {
        f64::from(width) / f64::from(display.width.max(1))
    });
    let points = 1.0 / density;
    let (width, height) = (
        (f64::from(display.width) * density).round() as u32,
        (f64::from(display.height) * density).round() as u32,
    );
    let capture = capture::start(display.id, width, height)?;
    let started = Instant::now();
    let ms = |at: Instant| -> f64 {
        match at.checked_duration_since(started) {
            Some(after) => after.as_secs_f64() * 1000.0,
            None => -started.duration_since(at).as_secs_f64() * 1000.0,
        }
    };
    let mut log = std::io::BufWriter::new(std::fs::File::create(out.join("events.jsonl"))?);
    let post = |kind: CGEventType, x: f64, y: f64| {
        let event = CGEvent::new_mouse_event(
            None,
            kind,
            CGPoint {
                x: x * points,
                y: y * points,
            },
            CGMouseButton::Left,
        );
        CGEvent::post(CGEventTapLocation::HIDEventTap, event.as_deref());
    };

    // The page, once it has stayed put for `STEADY`: an application still
    // placing its window, or going full screen, moves it.
    const STEADY: Duration = Duration::from_secs(1);
    // The column unread for this long means the picture moved or is covered.
    const LOST: Duration = Duration::from_millis(500);
    #[derive(Debug)]
    struct Moved;
    impl std::fmt::Display for Moved {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("the picture moved")
        }
    }
    impl std::error::Error for Moved {}
    let same = |a: &Mapping, b: &Mapping| {
        (a.offset_x - b.offset_x).abs() <= 1.5
            && (a.offset_y - b.offset_y).abs() <= 1.5
            && (a.scale_x / b.scale_x - 1.0).abs() <= 0.002
            && (a.scale_y / b.scale_y - 1.0).abs() <= 0.002
    };
    let lock = || -> Result<Mapping, Box<dyn std::error::Error>> {
        {
            let mut want = capture
                .want
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            want.column = None;
            want.card = None;
        }
        let deadline = Instant::now() + Duration::from_secs(wait);
        let mut held: Option<(Mapping, Instant)> = None;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(
                    "the perceived page did not appear, or did not stay put, on the main display"
                        .into(),
                );
            }
            let Ok(picture) = capture
                .pictures
                .recv_timeout(remaining.min(Duration::from_millis(200)))
            else {
                continue;
            };
            let Some(mapping) = locate(&picture.column) else {
                held = None;
                continue;
            };
            match held {
                Some((first, since)) if same(&first, &mapping) => {
                    if since.elapsed() >= STEADY {
                        return Ok(mapping);
                    }
                }
                _ => held = Some((mapping, Instant::now())),
            }
        }
    };
    // Where the top-left anchor reads inside the column, with its tolerance,
    // and when it last read there.
    let home = std::cell::Cell::new((0.0f64, 0.0f64, 0.0f64));
    let last_seen = std::cell::Cell::new(Instant::now());
    let crops = std::cell::Cell::new(0u32);
    type Each<'a> = dyn FnMut(
            &mut dyn Write,
            f64,
            &Reading,
            Option<&RawFrame>,
        ) -> Result<(), Box<dyn std::error::Error>>
        + 'a;
    // Reads every new picture until `until`, logging what the column says;
    // `each` sees every reading, and a card crop when one was asked for.
    // Fails with `Moved` once the column has not read in place for `LOST`.
    let observe = |log: &mut dyn Write,
                   until: Instant,
                   each: &mut Each|
     -> Result<(), Box<dyn std::error::Error>> {
        while let Some(remaining) = until.checked_duration_since(Instant::now()) {
            let Ok(picture) = capture.pictures.recv_timeout(remaining) else {
                continue;
            };
            let work = Instant::now();
            let reading = read(&picture.column);
            let (x, y, tolerance) = home.get();
            if reading.anchor.is_some_and(|anchor| {
                (f64::from(anchor.x) - x).abs() <= tolerance
                    && (f64::from(anchor.y) - y).abs() <= tolerance
            }) {
                last_seen.set(work);
            } else if work.duration_since(last_seen.get()) > LOST {
                return Err(Box::new(Moved));
            }
            let at = picture.column.captured_at().map_or(f64::NAN, ms);
            let field = |role: Role| {
                reading
                    .value(role)
                    .map_or("null".to_owned(), |value| value.to_string())
            };
            writeln!(
                log,
                r#"{{"type":"frame","t":{at:.3},"frame":{},"clicks":{},"x":{},"y":{},"card":{}}}"#,
                field(Role::Frame),
                field(Role::Clicks),
                field(Role::PointerX),
                field(Role::PointerY),
                field(Role::Card),
            )?;
            each(log, at, &reading, picture.card.as_ref())?;
            writeln!(
                log,
                r#"{{"type":"work","ms":{:.3},"lockedMs":{:.3}}}"#,
                work.elapsed().as_secs_f64() * 1000.0,
                picture.locked_ms,
            )?;
        }
        Ok(())
    };
    let mut idle = |_: &mut dyn Write, _: f64, _: &Reading, _: Option<&RawFrame>| Ok(());

    // Every phase, against one placement of the picture.
    let mut run = |mapping: Mapping,
                   log: &mut dyn Write|
     -> Result<(), Box<dyn std::error::Error>> {
        let card = mapping.card();
        // Viewer pixels for a host point, rounded to the point the event takes.
        let at = |hx: f64, hy: f64| -> (f64, f64) {
            let (vx, vy) = mapping.to_viewer(hx, hy);
            (
                (vx * points).round() / points,
                (vy * points).round() / points,
            )
        };
        let (host_width, host_height) = (f64::from(mapping.width), f64::from(mapping.height));

        // Pointer accuracy: jumps across a grid, each left to settle.
        observe(
            &mut *log,
            Instant::now() + Duration::from_millis(500),
            &mut idle,
        )?;
        for gy in 1..=4 {
            for gx in 1..=6 {
                let (vx, vy) = at(
                    host_width * f64::from(gx) / 7.0,
                    host_height * f64::from(gy) / 5.0,
                );
                let (hx, hy) = mapping.to_host(vx, vy);
                post(CGEventType::MouseMoved, vx, vy);
                writeln!(
                    log,
                    r#"{{"type":"move","kind":"jump","t":{:.3},"hostX":{hx:.2},"hostY":{hy:.2}}}"#,
                    ms(Instant::now())
                )?;
                observe(
                    &mut *log,
                    Instant::now() + Duration::from_millis(400),
                    &mut idle,
                )?;
            }
        }

        // Pointer under motion: a circle at 120 events a second for 3 s.
        let (cx, cy, radius) = (host_width / 2.0, host_height / 2.0, host_height / 4.0);
        let path_start = Instant::now();
        let mut step = 0u32;
        while path_start.elapsed() < Duration::from_secs(3) {
            let due = path_start + Duration::from_micros(u64::from(step) * 8_333);
            if Instant::now() >= due {
                let angle = f64::from(step) / 120.0 * std::f64::consts::TAU;
                let (vx, vy) = at(cx + radius * angle.cos(), cy + radius * angle.sin());
                let (hx, hy) = mapping.to_host(vx, vy);
                post(CGEventType::MouseMoved, vx, vy);
                writeln!(
                    log,
                    r#"{{"type":"move","kind":"path","t":{:.3},"hostX":{hx:.2},"hostY":{hy:.2}}}"#,
                    ms(Instant::now())
                )?;
                step += 1;
            }
            observe(
                &mut *log,
                Instant::now() + Duration::from_millis(1),
                &mut idle,
            )?;
        }
        observe(
            &mut *log,
            Instant::now() + Duration::from_millis(500),
            &mut idle,
        )?;

        // Click to photon: twenty clicks on the card.
        let (vx, vy) = at(host_width * 0.6, host_height * 0.4);
        post(CGEventType::MouseMoved, vx, vy);
        observe(
            &mut *log,
            Instant::now() + Duration::from_millis(300),
            &mut idle,
        )?;
        for _ in 0..20 {
            post(CGEventType::LeftMouseDown, vx, vy);
            post(CGEventType::LeftMouseUp, vx, vy);
            writeln!(log, r#"{{"type":"click","t":{:.3}}}"#, ms(Instant::now()))?;
            observe(
                &mut *log,
                Instant::now() + Duration::from_millis(400),
                &mut idle,
            )?;
        }

        // Pacing and sharpness: the pointer rests on the moving band, out of
        // the card, while every picture is read and the card is cropped at set
        // times after each swap.
        let cell = f64::from(mapping.cell);
        let (vx, vy) = at(14.0 * cell, host_height - 10.0 * cell);
        post(CGEventType::MouseMoved, vx, vy);
        const AFTER_MS: [f64; 12] = [
            0.0, 100.0, 250.0, 500.0, 750.0, 1000.0, 1500.0, 2000.0, 3000.0, 4000.0, 6000.0, 7500.0,
        ];
        let mut swap: Option<(u32, f64)> = None;
        let mut known: Option<u32> = None;
        let mut taken = 0usize;
        let mut asked = false;
        let want = std::sync::Arc::clone(&capture.want);
        let directory = out.clone();
        let mut sharpness = |log: &mut dyn Write,
                             now: f64,
                             reading: &Reading,
                             piece: Option<&RawFrame>|
         -> Result<(), Box<dyn std::error::Error>> {
            if let (Some(piece), Some((generation, since))) = (piece, swap) {
                let file = format!("crop-{}.bgra", crops.get());
                std::fs::write(directory.join(&file), piece.pixels())?;
                writeln!(
                    log,
                    r#"{{"type":"crop","t":{now:.3},"card":{generation},"sinceMs":{:.3},"file":"{file}","width":{},"height":{}}}"#,
                    now - since,
                    piece.width(),
                    piece.height(),
                )?;
                crops.set(crops.get() + 1);
                asked = false;
            }
            let Some(generation) = reading.value(Role::Card) else {
                return Ok(());
            };
            if known.is_some_and(|known| known != generation) {
                swap = Some((generation, now));
                taken = 0;
            }
            known = Some(generation);
            if let Some((_, since)) = swap {
                if !asked && taken < AFTER_MS.len() && now - since >= AFTER_MS[taken] {
                    want.lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .card = Some(card);
                    asked = true;
                    taken += 1;
                }
            }
            Ok(())
        };
        observe(
            &mut *log,
            Instant::now() + Duration::from_secs(20),
            &mut sharpness,
        )?;
        Ok(())
    };

    loop {
        let mapping = lock()?;
        let column = mapping.column();
        let card = mapping.card();
        // The anchor's cell 0 is 4 host cells in and its middle row 3.5
        // down; the column crop starts one host cell in.
        let cell = f64::from(mapping.cell);
        let (anchor_x, anchor_y) = mapping.to_viewer(4.0 * cell, 3.5 * cell);
        home.set((
            anchor_x - column.x.max(0.0).floor(),
            anchor_y - column.y.max(0.0).floor(),
            cell * mapping.scale_x / 2.0 + 2.0,
        ));
        last_seen.set(Instant::now());
        capture
            .want
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .column = Some(column);
        writeln!(
            log,
            r#"{{"type":"located","t":{:.3},"scaleX":{},"scaleY":{},"offsetX":{},"offsetY":{},"cell":{},"hostWidth":{},"hostHeight":{},"viewerWidth":{width},"viewerHeight":{height},"pointsPerPixel":{points},"cardX":{},"cardY":{},"cardWidth":{},"cardHeight":{}}}"#,
            ms(Instant::now()),
            mapping.scale_x,
            mapping.scale_y,
            mapping.offset_x,
            mapping.offset_y,
            mapping.cell,
            mapping.width,
            mapping.height,
            card.x,
            card.y,
            card.width,
            card.height,
        )?;
        match run(mapping, &mut log) {
            Ok(()) => break,
            Err(error) if error.is::<Moved>() => {
                writeln!(log, r#"{{"type":"moved","t":{:.3}}}"#, ms(Instant::now()))?;
                eprintln!("perceived: the picture moved; measuring again once it stays put");
            }
            Err(error) => return Err(error),
        }
    }
    log.flush()?;
    println!(
        "perceived: {} card crops, events in {}",
        crops.get(),
        out.join("events.jsonl").display()
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("perceived drives a macOS display");
    std::process::exit(2);
}
