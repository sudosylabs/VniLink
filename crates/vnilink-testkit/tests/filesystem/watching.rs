//! Watched folders: a change made by anything is reported until the folder
//! is unwatched, and VniLink's own temporary entries are neither listed nor
//! a change.

use std::time::Duration;

use vnilink_testkit::{traced, Scratch, TestResult};

use vnilink_testkit::files::{no_change, settle, wait_for_change, Location};

/// How long a change takes to be reported, at most.
const REPORTED: Duration = Duration::from_secs(5);
/// How long a folder is watched to see that something is not reported.
const UNREPORTED: Duration = Duration::from_millis(1_500);

/// A watched folder on this computer says when an entry comes or goes,
/// made by anything, and stops once unwatched.
#[test]
fn a_watched_folder_says_when_it_changes() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let folder = scratch.join("watched");
    std::fs::create_dir(&folder)?;
    let engine = vnilink_testkit::Engine::start("engine");
    let core = &engine.core;
    let location = Location::of(core, &folder);
    let events = core.subscribe()?;
    let watch = core.watch_folder(location.root, &location.path(""))?;
    // FSEvents may report the watch's own start; let it settle first.
    settle(&events);
    std::fs::write(folder.join("arrived.txt"), b"new")?;
    wait_for_change(&events, watch, "creating a file to be reported", REPORTED);
    // A late report of the creation must not pass for the deletion's.
    settle(&events);
    std::fs::remove_file(folder.join("arrived.txt"))?;
    wait_for_change(&events, watch, "deleting a file to be reported", REPORTED);
    core.unwatch_folder(watch);
    settle(&events);
    std::fs::write(folder.join("later.txt"), b"later")?;
    no_change(
        &events,
        watch,
        "an unwatched folder reporting no change",
        UNREPORTED,
    );
    Ok(())
}

/// VniLink's own temporary entries are never listed, and writing them is
/// not a change to the folder: a pane neither shows `.vnlpub.0.tmp` while
/// a copy is staged beside its destination, nor lists its folder again for
/// it. The rename that lands the file is a change.
#[test]
fn temporary_entries_are_never_listed_or_watched() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let folder = scratch.join("landing");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("kept.txt"), b"kept")?;
    for name in [
        ".vnlpub.0.tmp",
        ".vnlpart.7.3",
        ".vnlcase.1.tmp",
        ".vnldelete.0f",
    ] {
        std::fs::write(folder.join(name), b"temp")?;
    }
    // Names that only resemble them are the user's.
    std::fs::write(folder.join(".vnlrc"), b"mine")?;
    std::fs::write(folder.join("pairing.vnl"), b"mine")?;
    let engine = vnilink_testkit::Engine::start("engine");
    let core = &engine.core;
    let location = Location::of(core, &folder);
    let page = core.list_dir(location.root, &location.path(""), 0, 50)?;
    let names: Vec<&str> = page.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        [".vnlrc", "kept.txt", "pairing.vnl"],
        "only the user's entries are listed"
    );
    assert_eq!(page.total, 3, "only the user's entries are counted");
    assert_eq!(
        page.non_utf8_skipped, 0,
        "temporary entries are not counted as skipped"
    );

    let events = core.subscribe()?;
    let watch = core.watch_folder(location.root, &location.path(""))?;
    // The watch may report its own start; let it settle first.
    settle(&events);
    let staged = folder.join(".vnlpub.1.tmp");
    std::fs::write(&staged, vec![7u8; 64 * 1024])?;
    // kqueue on macOS names no entry reliably, so only where the watch
    // names what changed are writes to a temporary entry left out.
    #[cfg(not(target_os = "macos"))]
    {
        settle(&events);
        for _ in 0..4 {
            std::fs::OpenOptions::new()
                .append(true)
                .open(&staged)
                .and_then(|mut file| std::io::Write::write_all(&mut file, b"more"))?;
            no_change(
                &events,
                watch,
                "writing a temporary entry reporting no change",
                Duration::from_millis(100),
            );
        }
        no_change(
            &events,
            watch,
            "writing a temporary entry reporting no change",
            UNREPORTED,
        );
    }
    settle(&events);
    std::fs::rename(&staged, folder.join("landed.bin"))?;
    wait_for_change(&events, watch, "landing the file to be reported", REPORTED);
    core.unwatch_folder(watch);
    Ok(())
}
