// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Segments that shrink travel compressed and the rest travel as they are;
//! either way the file lands exactly, and a resume continues inside a
//! compressed segment.

use expect_test::expect;
use std::time::Duration;

use vnilink::JobState;
use vnilink_testkit::files::{drive_to_end, payload, received, tree, upload};
use vnilink_testkit::{traced, Pair, TestResult};

const TRANSFER: Duration = Duration::from_secs(60);

/// One segment of the file protocol.
const SEGMENT: usize = 16 * 1024 * 1024;

fn text(len: usize) -> Vec<u8> {
    b"a line of text that repeats, as logs and sources do\n"
        .iter()
        .copied()
        .cycle()
        .take(len)
        .collect()
}

/// Text sends compressed, noise sends as it is, and a file whose segments
/// differ decides for each: the file lands exactly, and the sender records
/// how many segments it compressed.
#[test]
fn compressible_segments_travel_compressed_and_the_rest_as_they_are() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let source = pair.controller.folder().join("set");
    std::fs::create_dir_all(&source)?;
    std::fs::write(source.join("log.txt"), text(SEGMENT + SEGMENT / 2))?;
    std::fs::write(source.join("noise.bin"), payload(3 * 1024 * 1024, 7))?;
    let mut mixed = payload(SEGMENT, 8);
    mixed.extend(text(2 * 1024 * 1024));
    std::fs::write(source.join("mixed.bin"), mixed)?;
    std::fs::write(source.join("tiny.txt"), text(300))?;
    let files = pair.open_files();
    let job = upload(&pair, &files, "set", "arrived")?;
    let receiver = received(&pair.host.core);
    let (sent, got) = drive_to_end(
        &pair.controller.core,
        job,
        &pair.host.core,
        receiver,
        TRANSFER,
    );
    assert_eq!(
        (sent.state, got.state),
        (JobState::Complete, JobState::Complete)
    );
    assert_eq!(
        tree(&pair.host.folder().join("arrived/set")),
        tree(&source),
        "every file lands exactly"
    );
    trace.wait_for_records("job state", 2);
    // By size: mixed.bin (one of its two segments), tiny.txt, log.txt (both
    // segments), noise.bin (none).
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served compressed=1 ended=sent from=0 sent=18874368 size=18874368
          files: file served compressed=1 ended=sent from=0 sent=300 size=300
          files: file served compressed=2 ended=sent from=0 sent=25165824 size=25165824
          files: file served ended=sent from=0 sent=3145728 size=3145728
          files: job state done_bytes=47186220 files_done=4 from=serving job=1 sender=true to=complete
        host:
          files: file received outcome=received resumed_at=0 size=18874368 took=18874368
          files: file received outcome=received resumed_at=0 size=25165824 took=25165824
          files: file received outcome=received resumed_at=0 size=300 took=300
          files: file received outcome=received resumed_at=0 size=3145728 took=3145728
          files: received files flushed job=1
          files: job state done_bytes=47186220 files_done=4 from=transferring job=1 sender=false to=complete
    "#]],
        || trace.concurrent_story(&["files"]),
    );
    Ok(())
}
