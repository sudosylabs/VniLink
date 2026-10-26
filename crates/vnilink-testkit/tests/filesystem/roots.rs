//! What an engine shares of its file system: nothing at first, then chosen
//! folders or the whole disk. Paths stay inside what is shared, however
//! they are spelled and whatever replaces a folder after it was approved.

use vnilink::{Error, FolderKind};
use vnilink_testkit::{traced, Engine, Scratch, TestResult};

use vnilink_testkit::files::{link_folder, Location};
use vnilink_testkit::store::Installed;

use super::{deep_folder, is_message, only_root, read, UNSAFE_PATH};

#[test]
fn whole_filesystem_browses_siblings_without_folder_configuration() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let left = scratch.join("left");
    let right = scratch.join("right");
    std::fs::create_dir(&left)?;
    std::fs::create_dir(&right)?;
    std::fs::write(right.join("note.txt"), b"sibling bytes")?;
    let engine = Installed::start("engine");
    let core = &engine.core;
    assert!(
        core.file_roots()?.is_empty(),
        "a new engine shares no folder"
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
    // Malformed wire paths and unknown roots are still rejected. Whole
    // filesystem access removes folder configuration, not input validation.
    for bad in ["..", "a/../b", "/absolute", "a//b"] {
        assert!(
            core.list_dir(right.root, bad, 0, 20).is_err(),
            "listing {bad:?} is refused"
        );
    }
    assert!(
        core.list_dir(u64::MAX, "", 0, 20).is_err(),
        "an unknown root is refused"
    );
    Ok(())
}

#[test]
fn roots_start_empty_and_jail_traversal() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let engine = Installed::start("engine");
    let core = &engine.core;
    assert!(
        core.file_roots()?.is_empty(),
        "a new engine shares no folder"
    );
    assert!(
        matches!(core.list_dir(0, "", 0, 10), Err(Error::Unsupported(_))),
        "an engine sharing nothing lists nothing"
    );

    let root = scratch.join("root");
    std::fs::create_dir(&root)?;
    core.set_file_roots(vec![root.clone()])?;
    let shared = only_root(core);

    for bad in [
        "..",
        "a/../../x",
        "/abs",
        "C:foo",
        "a\\b",
        "a/",
        "a//b",
        "a/./b",
    ] {
        assert!(
            core.list_dir(shared, bad, 0, 10).is_err(),
            "listing {bad:?} must be refused"
        );
    }
    assert!(
        matches!(core.list_dir(7, "", 0, 10), Err(Error::InvalidSetting(_))),
        "an unknown root is refused"
    );
    assert!(!scratch.join("x").exists(), "nothing escaped the root");

    // A link (a junction on Windows) is not an acceptable root.
    let link = scratch.join("linked-root");
    link_folder(&root, &link)?;
    assert!(
        core.set_file_roots(vec![link]).is_err(),
        "a link is refused as a root"
    );
    assert_eq!(
        core.file_roots()?.len(),
        1,
        "the good roots survive the failed call"
    );
    assert!(
        core.set_file_roots(vec![root.clone(), root]).is_err(),
        "the same root twice is refused"
    );
    Ok(())
}

#[test]
fn symlink_parent_replacement_is_refused() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let root = scratch.join("root");
    std::fs::create_dir_all(root.join("real/sub"))?;
    std::fs::write(root.join("real/sub/kept.txt"), b"kept")?;
    let outside = scratch.join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("evil.txt"), b"evil")?;

    let engine = Engine::start("engine");
    let core = &engine.core;
    core.set_file_roots(vec![root.clone()])?;
    let shared = only_root(core);
    assert_eq!(
        core.list_dir(shared, "real/sub", 0, 10)?.total,
        1,
        "the folder lists before it is replaced"
    );

    // Swap the parent for a link (a junction on Windows) after listing.
    std::fs::remove_dir_all(root.join("real"))?;
    link_folder(&outside, &root.join("real"))?;
    assert!(
        core.list_dir(shared, "real/sub", 0, 10).is_err(),
        "a listing through a link is refused"
    );
    assert!(
        core.create_dir(shared, "real", "nope").is_err(),
        "a folder made through a link is refused"
    );
    assert!(
        core.rename_entry(shared, "real", "evil.txt", None, "moved.txt")
            .is_err(),
        "a rename through a link is refused"
    );
    assert!(
        core.delete_entry(shared, "real", "evil.txt", None).is_err(),
        "a delete through a link is refused"
    );
    assert!(!root.join("moved.txt").exists(), "nothing moved");
    assert_eq!(
        read(&outside.join("evil.txt")),
        b"evil",
        "what is outside is untouched"
    );
    Ok(())
}

#[test]
fn approved_root_replaced_by_symlink_cannot_escape() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let approved = scratch.join("approved");
    std::fs::create_dir(&approved)?;
    let outside = scratch.join("outside");
    std::fs::create_dir(&outside)?;

    let engine = Engine::start("engine");
    let core = &engine.core;
    core.set_file_roots(vec![approved.clone()])?;
    let shared = only_root(core);
    assert!(
        core.list_dir(shared, "", 0, 10).is_ok(),
        "the approved folder lists"
    );

    // Replace the approved folder itself with a link (a junction on
    // Windows) to outside.
    std::fs::remove_dir(&approved)?;
    link_folder(&outside, &approved)?;

    // Every anchored operation must refuse instead of writing outside.
    assert!(
        core.list_dir(shared, "", 0, 10).is_err(),
        "a replaced root does not list"
    );
    assert!(
        core.create_dir(shared, "", "escape").is_err(),
        "a replaced root takes no folder"
    );
    assert!(!outside.join("escape").exists(), "nothing was made outside");
    assert!(
        core.rename_entry(shared, "", "anything", None, "else")
            .is_err(),
        "a replaced root renames nothing"
    );
    assert!(
        core.delete_entry(shared, "", "anything", None).is_err(),
        "a replaced root deletes nothing"
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
    let account = std::fs::canonicalize(
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .ok_or("an account folder")?,
    )?;
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

/// A whole disk is deeper than a shared folder: 40 levels below a scratch
/// folder still list, and paths stop at 64 levels.
#[test]
fn deep_folders_list_and_the_depth_limit_holds() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let deep = deep_folder(scratch.path(), 40);
    std::fs::write(deep.join("bottom.txt"), b"deep")?;
    let engine = Engine::start("engine");
    let core = &engine.core;
    core.enable_filesystem_access()?;
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
/// own name rather than its target's, and a path outside what is shared
/// is not named at all.
#[test]
fn a_dropped_path_is_named_by_its_folder_and_its_own_name() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let folder = scratch.join("dropped");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("note.txt"), b"note")?;
    let engine = Engine::start("engine");
    let core = &engine.core;
    core.enable_filesystem_access()?;
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
    let shared = scratch.join("shared");
    std::fs::create_dir(&shared)?;
    core.set_file_roots(vec![shared])?;
    assert_eq!(
        core.locate(&folder.join("note.txt"))?,
        None,
        "a path outside what is shared is not named"
    );
    Ok(())
}
