// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use std::time::{Duration, Instant};

use crate::clock::{host_time_instant, host_time_now, ticks_to_duration};

#[test]
fn host_ticks_convert_with_the_timebase() {
    // Apple silicon counts 24 MHz ticks: 125/3 ns each.
    assert_eq!(
        ticks_to_duration(24_000_000, (125, 3)),
        Duration::from_secs(1)
    );
    assert_eq!(ticks_to_duration(1_000, (1, 1)), Duration::from_micros(1));
    assert_eq!(
        ticks_to_duration(u64::MAX, (u32::MAX, 1)),
        Duration::from_nanos(u64::MAX)
    );
}

#[test]
fn a_host_time_maps_to_the_instant_it_names_and_the_future_is_refused() {
    let before = Instant::now();
    let at = host_time_instant(host_time_now()).unwrap();
    let after = Instant::now();
    assert!(at >= before.checked_sub(Duration::from_millis(1)).unwrap());
    assert!(at <= after);
    let ago = host_time_now() - 24_000_000;
    let earlier = host_time_instant(ago).unwrap();
    assert!(earlier < before);
    assert!(host_time_instant(host_time_now() + 24_000_000).is_none());
}
