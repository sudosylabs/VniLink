// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Diagnostic: how often ScreenCaptureKit reports the main display changed,
//! and how often its pixels did, over the whole display and with each
//! application that has windows on it left out in turn.
//!
//! `cargo run -p vnilink-macos --release --example sck_changes -- [seconds per probe]`

#[cfg(target_os = "macos")]
fn main() {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use apple_cf::cm::CMTime;
    use screencapturekit::cm::{
        CMSampleBuffer, CMSampleBufferExt, CMSampleBufferSCExt, SCFrameStatus,
    };
    use screencapturekit::shareable_content::SCShareableContent;
    use screencapturekit::stream::configuration::{PixelFormat, SCStreamConfiguration};
    use screencapturekit::stream::content_filter::SCContentFilter;
    use screencapturekit::stream::output_trait::SCStreamOutputTrait;
    use screencapturekit::stream::output_type::SCStreamOutputType;
    use screencapturekit::stream::SCStream;

    #[derive(Default)]
    struct Counts {
        complete: u32,
        changed: u32,
        /// Frames with dirty rects, and frames whose pixels changed without any.
        dirty: u32,
        missed: u32,
        last: Option<u64>,
    }
    struct Handler(Arc<Mutex<Counts>>);
    impl SCStreamOutputTrait for Handler {
        fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
            if of_type != SCStreamOutputType::Screen
                || sample.frame_status() != Some(SCFrameStatus::Complete)
            {
                return;
            }
            let Some(buffer) = sample.pixel_buffer() else {
                return;
            };
            let Ok(guard) = buffer.lock_read_only() else {
                return;
            };
            let Some(bytes) = (unsafe { guard.as_slice() }) else {
                return;
            };
            // FNV-1a over every byte: any pixel that changed changes it.
            let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
            });
            let dirty = sample.dirty_rects().is_some();
            let mut counts = self.0.lock().unwrap();
            counts.complete += 1;
            counts.dirty += u32::from(dirty);
            if counts.last != Some(hash) {
                counts.changed += 1;
                counts.missed += u32::from(!dirty);
            }
            counts.last = Some(hash);
        }
    }

    let seconds: u64 = std::env::args()
        .nth(1)
        .map_or(3, |text| text.parse().expect("seconds"));
    let content = SCShareableContent::get().expect("Screen Recording permission");
    let displays = content.displays();
    let display = displays.first().expect("a display");
    let probe = |filter: SCContentFilter| -> (u32, u32, u32, u32) {
        let counts = Arc::new(Mutex::new(Counts::default()));
        let configuration = SCStreamConfiguration::new()
            .with_queue_depth(5)
            .with_width(1920)
            .with_height(1080)
            .with_pixel_format(PixelFormat::BGRA)
            .with_shows_cursor(false)
            .with_minimum_frame_interval(&CMTime::new(1, 30));
        let mut stream = SCStream::new(&filter, &configuration).expect("stream");
        stream
            .add_output_handler(Handler(Arc::clone(&counts)), SCStreamOutputType::Screen)
            .expect("handler");
        stream.start_capture().expect("capture");
        // The first picture is a change; count from a quiet start.
        std::thread::sleep(Duration::from_millis(500));
        {
            let mut counts = counts.lock().unwrap();
            counts.complete = 0;
            counts.changed = 0;
            counts.dirty = 0;
            counts.missed = 0;
        }
        std::thread::sleep(Duration::from_secs(seconds));
        let _ = stream.stop_capture();
        let counts = counts.lock().unwrap();
        (counts.complete, counts.changed, counts.dirty, counts.missed)
    };
    let whole = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&[])
        .build()
        .expect("filter");
    let (complete, changed, dirty, missed) = probe(whole);
    println!("| Left out | Complete frames | Pixels changed | With dirty rects | Changed without | over {seconds} s |");
    println!("|---|---|---|---|---|---|");
    println!("| nothing | {complete} | {changed} | {dirty} | {missed} | |");
    let windows = content.windows();
    let mut seen = std::collections::BTreeSet::new();
    let mut applications = Vec::new();
    for window in windows.iter().filter(|window| window.is_on_screen()) {
        if let Some(application) = window.owning_application() {
            if seen.insert(application.process_id()) {
                applications.push(application);
            }
        }
    }
    for application in &applications {
        let filter = SCContentFilter::create()
            .with_display(display)
            .with_excluding_applications(&[application], &[])
            .build()
            .expect("filter");
        let (complete, changed, dirty, missed) = probe(filter);
        println!(
            "| {} ({}) | {complete} | {changed} | {dirty} | {missed} | |",
            application.application_name(),
            application.bundle_identifier()
        );
    }
    println!();
    println!("| Only | Complete frames | Pixels changed | With dirty rects | Changed without | over {seconds} s |");
    println!("|---|---|---|---|---|---|");
    for application in &applications {
        let filter = SCContentFilter::create()
            .with_display(display)
            .with_including_applications(&[application], &[])
            .build()
            .expect("filter");
        let (complete, changed, dirty, missed) = probe(filter);
        println!(
            "| {} ({}) | {complete} | {changed} | {dirty} | {missed} | |",
            application.application_name(),
            application.bundle_identifier()
        );
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {}
