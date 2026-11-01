// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use vnilink_media::Error;

use crate::encode::{MacEncoder, Presentation};

#[test]
fn an_odd_encoder_size_is_rejected() {
    assert!(matches!(
        MacEncoder::open(63, 64, 30, 500_000, false, vnilink::YuvMatrix::Bt709),
        Err(Error::InvalidFrame("H.264 width and height must be even"))
    ));
}

#[test]
fn presentation_times_follow_the_capture_and_never_repeat() {
    let origin = std::time::Instant::now();
    let mut presentation = Presentation::default();
    assert_eq!(
        presentation.next(Some(origin), 0),
        0,
        "the first picture opens the clock"
    );
    let late = origin + std::time::Duration::from_millis(40);
    assert_eq!(
        presentation.next(Some(late), 10),
        40_000,
        "a gap in capture is a gap in time"
    );
    assert_eq!(
        presentation.next(Some(late), 20),
        40_001,
        "the same capture time again still rises"
    );
    assert_eq!(
        presentation.next(Some(origin), 30),
        40_002,
        "a capture time that runs backwards cannot lower the time"
    );
}

#[test]
fn a_picture_without_a_capture_time_takes_the_time_it_was_read() {
    let mut presentation = Presentation::default();
    assert_eq!(presentation.next(None, 5_000), 5_000);
    assert_eq!(presentation.next(None, 21_000), 21_000);
}
