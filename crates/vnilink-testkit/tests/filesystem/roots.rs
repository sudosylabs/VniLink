//! What an engine shares of its file system: nothing at first, then the
//! whole disk. Paths are named by a root and a relative path that keeps to
//! the path rules, however they are spelled.

use vnilink::{Error, FolderKind};
use vnilink_testkit::{traced, Engine, Scratch, TestResult};

use vnilink_testkit::files::{link_folder, Location};
use vnilink_testkit::store::Installed;

use super::{deep_folder, is_message, UNSAFE_PATH};

/// An engine shares nothing until its disk is shared; then any folder on
/// it lists and takes a new folder, and a path that leaves the path rules
/// or an unknown root is refused.
#[test]
fn the_whole_disk_is_shared_on_request_and_paths_keep_the_rules() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let left = scratch.join("left");
    let right = scratch.join("right");
    std::fs::create_dir(&left)?;
    std::fs::create_dir(&right)?;
    std::fs::write(right.join("note.txt"), b"sibling bytes")?;
    let engine = Installed::start("engine");
    let core = &engine.core;
    assert!(core.file_roots()?.is_empty(), "a new engine shares nothing");
    assert!(
        matches!(core.list_dir(0, "", 0, 10), Err(Error::Unsupported(_))),
        "an engine sharing nothing lists nothing"
    );
    core.enable_filesystem_access()?;
    let left = Location::of(core, &left);
    let right = Location::of(core, &right);
    assert!(
        core.list_dir(left.root, &left.path(""), 0, 20)?
            .entries
            .is_empty(),
        "an empty folder lists nothing"
    );
    let page = core.list_dir(right.root, &right.path(""), 0, 20)?;
    assert_eq!(page.total, 1, "its sibling lists its one file");
    assert_eq!(
        page.entries[0].name, "note.txt",
        "its sibling lists its file"
    );
    core.create_dir(right.root, &right.path(""), "created")?;
    assert!(
        scratch.join("right/created").is_dir(),
        "a folder made on the whole disk is on disk"
    );
    // Malformed wire paths and unknown roots are rejected.
    for bad in [
        "..",
        "a/../b",
        "/absolute",
        "C:foo",
        "a\\b",
        "a/",
        "a//b",
        "a/./b",
    ] {
        assert!(
            core.list_dir(right.root, &right.path(bad), 0, 20).is_err(),
            "listing {bad:?} is refused"
        );
    }
    assert!(
        core.list_dir(u64::MAX, "", 0, 20).is_err(),
        "an unknown root is refused"
    );
    Ok(())
}

/// Home and its usual folders are named by root and path, and each opens.
#[test]
fn places_name_home_and_its_folders_by_root() -> TestResult {
    let _trace = traced();
    let engine = Installed::start("engine");
    let core = &engine.core;
    assert!(core.places()?.roots.is_empty(), "nothing is shared yet");
    core.enable_filesystem_access()?;
    let places = core.places()?;
    let roots: Vec<u64> = core.file_roots()?.iter().map(|root| root.0).collect();
    assert_eq!(
        places.roots.iter().map(|root| root.id).collect::<Vec<_>>(),
        roots,
        "the places' roots are the engine's roots"
    );
    #[cfg(unix)]
    assert_eq!(places.roots[0].name, "/", "the disk is named /");
    let Some(home) = places.folders.first() else {
        return Ok(());
    };
    assert_eq!(home.kind, FolderKind::Home, "home comes first");
    let account = std::fs::canonicalize(account_folder().ok_or("an account folder")?)?;
    let location = Location::of(core, &account);
    assert_eq!(
        (home.root, home.path.as_str()),
        (location.root, location.path("").as_str()),
        "home is named by its root and path"
    );
    for folder in &places.folders {
        assert!(
            core.list_dir(folder.root, &folder.path, 0, 1).is_ok(),
            "{:?} does not open",
            folder.kind
        );
    }
    Ok(())
}

/// A whole disk is deep: 40 levels below a scratch folder still list, and
/// paths stop at 64 levels.
#[test]
fn deep_folders_list_and_the_depth_limit_holds() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let deep = deep_folder(scratch.path(), 40);
    std::fs::write(deep.join("bottom.txt"), b"deep")?;
    let engine = Engine::start("engine");
    let core = &engine.core;
    let location = Location::of(core, &deep);
    let page = core.list_dir(location.root, &location.path(""), 0, 10)?;
    assert_eq!(
        page.entries[0].name, "bottom.txt",
        "a folder 40 levels below the scratch folder lists"
    );
    let too_deep = vec!["a"; 65].join("/");
    assert!(
        core.list_dir(location.root, &too_deep, 0, 10)
            .is_err_and(|error| is_message(&error, UNSAFE_PATH)),
        "a path of 65 levels is refused as unsafe"
    );
    // 64 levels are a path the engine walks: it is refused as missing, not
    // as unsafe.
    let deepest = vec!["a"; 64].join("/");
    assert!(
        core.list_dir(location.root, &deepest, 0, 10)
            .is_err_and(|error| !is_message(&error, UNSAFE_PATH)),
        "a path of 64 levels is not refused as unsafe"
    );
    Ok(())
}

/// A path from the system's file manager is named as the panes name an
/// item: its folder's root and path, and its own name. A link keeps its
/// own name rather than its target's, and an engine sharing nothing names
/// nothing.
#[test]
fn a_dropped_path_is_named_by_its_folder_and_its_own_name() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let folder = scratch.join("dropped");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("note.txt"), b"note")?;
    let engine = Engine::start("engine");
    let core = &engine.core;
    let location = Location::of(core, &folder);
    assert_eq!(
        core.locate(&folder.join("note.txt"))?,
        Some((location.root, location.path(""), "note.txt".into())),
        "a dropped file is named by its folder and its name"
    );
    std::fs::create_dir(folder.join("inner"))?;
    link_folder(&folder.join("inner"), &folder.join("shortcut"))?;
    assert_eq!(
        core.locate(&folder.join("shortcut"))?,
        Some((location.root, location.path(""), "shortcut".into())),
        "a dropped link keeps its own name"
    );
    let unshared = Installed::start("unshared");
    assert_eq!(
        unshared.core.locate(&folder.join("note.txt"))?,
        None,
        "an engine sharing nothing names nothing"
    );
    Ok(())
}

/// The account's own folder as the system records it, which `HOME` need
/// not name: a container's runner sets `HOME` to a folder of its own.
#[cfg(unix)]
fn account_folder() -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: getpwuid returns null or a record owned by libc, read before
    // any other call that could replace it.
    unsafe {
        let record = libc::getpwuid(libc::geteuid());
        if record.is_null() || (*record).pw_dir.is_null() {
            return None;
        }
        let folder = std::ffi::CStr::from_ptr((*record).pw_dir);
        Some(std::ffi::OsStr::from_bytes(folder.to_bytes()).into())
    }
}

#[cfg(windows)]
fn account_folder() -> Option<std::path::PathBuf> {
    std::env::var_os("USERPROFILE").map(Into::into)
}
