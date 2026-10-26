//! Changes made in place: folders made, entries renamed, deleted or moved
//! to the Trash. A change names the entry as it was listed, and is refused
//! when what is there now is another object; nothing is left beside it.

use vnilink::Error;
use vnilink_testkit::{traced, Engine, Scratch, TestResult};

use vnilink_testkit::files::{identity, set_aside, token_of, Location};

use super::{is_message, is_stale, names_in, past_the_tick_of, read, DELETE_FAILED};

/// An engine with its whole file system shared, and `folder` addressed in
/// it.
fn sharing_the_disk(folder: &std::path::Path) -> Result<(Engine, Location), Error> {
    let engine = Engine::start("engine");
    engine.core.enable_filesystem_access()?;
    let location = Location::of(&engine.core, folder);
    Ok((engine, location))
}

#[test]
fn mkdir_creates_single_levels_with_conflicts() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let root = scratch.join("root");
    std::fs::create_dir(&root)?;
    let (engine, files) = sharing_the_disk(&root)?;
    let core = &engine.core;

    core.create_dir(files.root, &files.path(""), "docs")?;
    assert!(root.join("docs").is_dir(), "the folder is made");
    core.create_dir(files.root, &files.path("docs"), "2026")?;
    assert!(
        root.join("docs/2026").is_dir(),
        "a folder is made inside it"
    );

    assert!(
        matches!(
            core.create_dir(files.root, &files.path(""), "docs"),
            Err(Error::FileExists)
        ),
        "a folder already there is not made again"
    );
    std::fs::write(root.join("file.txt"), b"f")?;
    assert!(
        matches!(
            core.create_dir(files.root, &files.path(""), "file.txt"),
            Err(Error::FileExists)
        ),
        "a folder is not made over a file"
    );
    assert!(
        core.create_dir(files.root, &files.path(""), "a/b").is_err(),
        "one level at a time"
    );
    assert!(
        core.create_dir(files.root, &files.path(""), "..").is_err(),
        "a folder named .. is refused"
    );
    assert!(
        core.create_dir(files.root, &files.path("missing-parent"), "x")
            .is_err(),
        "a folder is not made in a missing one"
    );
    Ok(())
}

#[test]
fn rename_file_keeps_identity_and_bytes() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let root = scratch.join("root");
    std::fs::create_dir(&root)?;
    std::fs::write(root.join("before.txt"), b"vnl-rename-bytes")?;
    let (engine, files) = sharing_the_disk(&root)?;
    let core = &engine.core;

    let before_ino = identity(&root.join("before.txt"));
    let token = token_of(core, files.root, &files.path(""), "before.txt");
    core.rename_entry(
        files.root,
        &files.path(""),
        "before.txt",
        Some(token),
        "after.txt",
    )?;

    assert!(!root.join("before.txt").exists(), "the old name is gone");
    assert_eq!(
        read(&root.join("after.txt")),
        b"vnl-rename-bytes",
        "the bytes are kept"
    );
    assert_eq!(
        identity(&root.join("after.txt")),
        before_ino,
        "the file is the same object"
    );
    assert_eq!(
        names_in(&root),
        ["after.txt"],
        "no temporary or duplicate sibling is left"
    );
    Ok(())
}

#[test]
fn rename_folder_moves_children_without_transfer() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let root = scratch.join("root");
    std::fs::create_dir_all(root.join("pack/sub"))?;
    std::fs::create_dir(root.join("pack/empty"))?;
    std::fs::write(root.join("pack/sub/a.txt"), b"a-bytes")?;
    std::fs::write(root.join("pack/top.txt"), b"top-bytes")?;
    let (engine, files) = sharing_the_disk(&root)?;
    let core = &engine.core;

    let child = identity(&root.join("pack/sub/a.txt"));
    let token = token_of(core, files.root, &files.path(""), "pack");
    core.rename_entry(files.root, &files.path(""), "pack", Some(token), "box")?;

    assert!(!root.join("pack").exists(), "the old name is gone");
    assert!(root.join("box/empty").is_dir(), "an empty child moves");
    assert_eq!(
        read(&root.join("box/sub/a.txt")),
        b"a-bytes",
        "a nested child moves with its bytes"
    );
    assert_eq!(
        read(&root.join("box/top.txt")),
        b"top-bytes",
        "a child moves with its bytes"
    );
    assert_eq!(
        identity(&root.join("box/sub/a.txt")),
        child,
        "a child is the same object, not a copy"
    );
    Ok(())
}

#[test]
fn rename_collision_after_preflight_preserves_both() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let root = scratch.join("root");
    std::fs::create_dir(&root)?;
    std::fs::write(root.join("source.txt"), b"source-bytes")?;
    let (engine, files) = sharing_the_disk(&root)?;
    let core = &engine.core;

    // Preflight: the destination is free when first checked.
    let token = token_of(core, files.root, &files.path(""), "source.txt");
    // Another actor creates the destination before the commit.
    std::fs::write(root.join("dest.txt"), b"dest-bytes")?;

    let result = core.rename_entry(
        files.root,
        &files.path(""),
        "source.txt",
        Some(token),
        "dest.txt",
    );
    assert!(
        matches!(result, Err(Error::FileExists)),
        "a rename onto an entry made meanwhile is refused"
    );
    assert_eq!(
        read(&root.join("source.txt")),
        b"source-bytes",
        "the source is kept"
    );
    assert_eq!(
        read(&root.join("dest.txt")),
        b"dest-bytes",
        "the other entry is kept"
    );
    Ok(())
}

#[test]
fn rename_against_a_replaced_source_is_refused() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let root = scratch.join("root");
    std::fs::create_dir(&root)?;
    std::fs::write(root.join("note.txt"), b"original-bytes")?;
    let (engine, files) = sharing_the_disk(&root)?;
    let core = &engine.core;

    let stale = token_of(core, files.root, &files.path(""), "note.txt");
    // Barrier-controlled replacement between list and mutation: the
    // replacement is born while the original is still alive, so the two
    // objects cannot share an identity even where the filesystem
    // recycles inode numbers immediately after unlink.
    std::fs::write(root.join("note.tmp"), b"replacement-bytes")?;
    let fresh = token_of(core, files.root, &files.path(""), "note.tmp");
    assert_ne!(stale, fresh, "two live objects have two identities");
    std::fs::remove_file(root.join("note.txt"))?;
    std::fs::rename(root.join("note.tmp"), root.join("note.txt"))?;
    assert_eq!(
        token_of(core, files.root, &files.path(""), "note.txt"),
        fresh,
        "the name now lists the replacement"
    );

    let result = core.rename_entry(
        files.root,
        &files.path(""),
        "note.txt",
        Some(stale),
        "renamed.txt",
    );
    assert!(
        result.as_ref().is_err_and(is_stale),
        "a rename of the listed object is refused once replaced: {result:?}"
    );
    assert!(!root.join("renamed.txt").exists(), "nothing was renamed");
    assert_eq!(
        read(&root.join("note.txt")),
        b"replacement-bytes",
        "the replacement is kept"
    );

    // Without a fencing token the current object renames (the UI always
    // passes the listing token; unfenced callers own the race).
    core.rename_entry(files.root, &files.path(""), "note.txt", None, "renamed.txt")?;
    assert_eq!(
        read(&root.join("renamed.txt")),
        b"replacement-bytes",
        "an unfenced rename renames what is there"
    );
    Ok(())
}

#[test]
fn rename_missing_source_and_bad_names_fail_cleanly() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let root = scratch.join("root");
    std::fs::create_dir(&root)?;
    std::fs::write(root.join("kept.txt"), b"kept")?;
    let (engine, files) = sharing_the_disk(&root)?;
    let core = &engine.core;

    assert!(
        core.rename_entry(files.root, &files.path(""), "ghost.txt", None, "x.txt")
            .is_err(),
        "a missing source is refused"
    );
    assert!(
        core.rename_entry(files.root, &files.path(""), "kept.txt", None, "../out.txt")
            .is_err(),
        "a new name leaving the folder is refused"
    );
    assert!(
        core.rename_entry(files.root, &files.path(""), "kept.txt", None, "sub/x.txt")
            .is_err(),
        "a new name with a separator is refused"
    );
    assert!(
        core.rename_entry(files.root, &files.path(""), "../kept.txt", None, "x.txt")
            .is_err(),
        "a source outside the folder is refused"
    );
    assert_eq!(read(&root.join("kept.txt")), b"kept", "the file is kept");
    assert!(!scratch.join("out.txt").exists(), "nothing left the folder");
    Ok(())
}

#[test]
fn case_only_rename_keeps_the_object() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let root = scratch.join("root");
    std::fs::create_dir(&root)?;
    std::fs::write(root.join("Case.txt"), b"case-bytes")?;
    let (engine, files) = sharing_the_disk(&root)?;
    let core = &engine.core;

    let before = identity(&root.join("Case.txt"));
    let token = token_of(core, files.root, &files.path(""), "Case.txt");
    match core.rename_entry(
        files.root,
        &files.path(""),
        "Case.txt",
        Some(token),
        "CASE.txt",
    ) {
        Ok(()) => {
            assert_eq!(
                read(&root.join("CASE.txt")),
                b"case-bytes",
                "the bytes are kept"
            );
            assert_eq!(
                identity(&root.join("CASE.txt")),
                before,
                "the file is the same object"
            );
        }
        Err(Error::FileExists) => {
            // Case-sensitive filesystem treating the spellings as colliding
            // with a distinct entry, or a platform refusing in place: the
            // original must survive either way.
            assert_eq!(
                read(&root.join("Case.txt")),
                b"case-bytes",
                "a refused case-only rename keeps the original"
            );
        }
        Err(other) => panic!("unexpected case-only outcome: {other}"),
    }
    Ok(())
}

#[test]
fn delete_removes_a_file_and_a_folder_tree() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let root = scratch.join("root");
    std::fs::create_dir_all(root.join("pack/sub"))?;
    std::fs::create_dir(root.join("pack/empty"))?;
    std::fs::write(root.join("pack/sub/a.txt"), b"a-bytes")?;
    std::fs::write(root.join("pack/top.txt"), b"top-bytes")?;
    std::fs::write(root.join("note.txt"), b"note-bytes")?;
    let (engine, files) = sharing_the_disk(&root)?;
    let core = &engine.core;

    let note = token_of(core, files.root, &files.path(""), "note.txt");
    core.delete_entry(files.root, &files.path(""), "note.txt", Some(note))?;
    assert!(!root.join("note.txt").exists(), "the file is deleted");

    let folder = token_of(core, files.root, &files.path(""), "pack");
    core.delete_entry(files.root, &files.path(""), "pack", Some(folder))?;
    assert!(!root.join("pack").exists(), "the folder tree is deleted");
    assert!(set_aside(&root).is_empty(), "nothing is left set aside");
    let names = names_in(&root);
    assert!(names.is_empty(), "nothing is left: {names:?}");
    Ok(())
}

#[test]
fn delete_against_a_replaced_source_is_refused() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let root = scratch.join("root");
    std::fs::create_dir(&root)?;
    std::fs::write(root.join("note.txt"), b"original-bytes")?;
    let (engine, files) = sharing_the_disk(&root)?;
    let core = &engine.core;

    let stale = token_of(core, files.root, &files.path(""), "note.txt");
    std::fs::write(root.join("note.tmp"), b"replacement-bytes")?;
    let fresh = token_of(core, files.root, &files.path(""), "note.tmp");
    assert_ne!(stale, fresh, "two live objects have two identities");
    std::fs::remove_file(root.join("note.txt"))?;
    std::fs::rename(root.join("note.tmp"), root.join("note.txt"))?;

    let result = core.delete_entry(files.root, &files.path(""), "note.txt", Some(stale));
    assert!(
        result.as_ref().is_err_and(is_stale),
        "a delete of the listed object is refused once replaced: {result:?}"
    );
    assert_eq!(
        read(&root.join("note.txt")),
        b"replacement-bytes",
        "the replacement is kept"
    );
    assert!(set_aside(&root).is_empty(), "nothing is left set aside");

    core.delete_entry(files.root, &files.path(""), "note.txt", None)?;
    assert!(
        !root.join("note.txt").exists(),
        "an unfenced delete deletes what is there"
    );
    Ok(())
}

#[test]
fn delete_missing_source_and_bad_names_fail_cleanly() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let root = scratch.join("root");
    std::fs::create_dir(&root)?;
    std::fs::write(root.join("kept.txt"), b"kept")?;
    let (engine, files) = sharing_the_disk(&root)?;
    let core = &engine.core;

    let missing = core.delete_entry(files.root, &files.path(""), "ghost.txt", None);
    assert!(
        missing
            .as_ref()
            .is_err_and(|error| is_message(error, DELETE_FAILED)),
        "a missing entry could not be deleted: {missing:?}"
    );
    assert!(
        core.delete_entry(files.root, &files.path(""), "../kept.txt", None)
            .is_err(),
        "an entry outside the folder is refused"
    );
    assert!(
        core.delete_entry(files.root, &files.path(""), "sub/x.txt", None)
            .is_err(),
        "a name with a separator is refused"
    );
    assert_eq!(read(&root.join("kept.txt")), b"kept", "the file is kept");
    assert!(set_aside(&root).is_empty(), "nothing is left set aside");
    assert!(
        !scratch.join("kept.txt").exists(),
        "nothing left the folder"
    );
    Ok(())
}

#[test]
#[cfg(unix)]
fn delete_refuses_a_listed_symlink() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let root = scratch.join("root");
    let outside = scratch.join("outside");
    std::fs::create_dir_all(&root)?;
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("secret.txt"), b"secret-bytes")?;
    std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link"))?;
    let (engine, files) = sharing_the_disk(&root)?;
    let core = &engine.core;

    let token = token_of(core, files.root, &files.path(""), "link");
    let result = core.delete_entry(files.root, &files.path(""), "link", Some(token));
    assert!(
        result
            .as_ref()
            .is_err_and(|error| is_message(error, DELETE_FAILED)),
        "a link could not be deleted: {result:?}"
    );
    assert!(
        std::fs::symlink_metadata(root.join("link"))?
            .file_type()
            .is_symlink(),
        "the link is kept"
    );
    assert_eq!(
        read(&outside.join("secret.txt")),
        b"secret-bytes",
        "its target is untouched"
    );
    Ok(())
}

#[test]
#[cfg(unix)]
fn delete_folder_unlinks_symlink_children_without_following() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let root = scratch.join("root");
    let outside = scratch.join("outside");
    std::fs::create_dir_all(root.join("pack"))?;
    std::fs::create_dir_all(outside.join("dir"))?;
    std::fs::write(outside.join("secret.txt"), b"secret-bytes")?;
    std::fs::write(outside.join("dir/nested.txt"), b"nested-bytes")?;
    std::fs::write(root.join("pack/real.txt"), b"real-bytes")?;
    std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("pack/link"))?;
    std::os::unix::fs::symlink(outside.join("dir"), root.join("pack/dirlink"))?;
    let (engine, files) = sharing_the_disk(&root)?;
    let core = &engine.core;

    let token = token_of(core, files.root, &files.path(""), "pack");
    core.delete_entry(files.root, &files.path(""), "pack", Some(token))?;
    assert!(!root.join("pack").exists(), "the folder is deleted");
    assert!(set_aside(&root).is_empty(), "nothing is left set aside");
    assert_eq!(
        read(&outside.join("secret.txt")),
        b"secret-bytes",
        "a linked file is not followed"
    );
    assert_eq!(
        read(&outside.join("dir/nested.txt")),
        b"nested-bytes",
        "a linked folder is not followed"
    );
    Ok(())
}

/// Moving to the Trash checks the entry as deleting does: one replaced
/// since it was listed is refused before anything reaches the Trash, and
/// so is a name that is not there. (The move itself would use this
/// account's real Trash, so it is left to manual checks.)
#[test]
fn moving_to_the_trash_refuses_what_changed_since_it_was_listed() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let folder = scratch.join("trash");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("a.txt"), b"first")?;
    let (engine, location) = sharing_the_disk(&folder)?;
    let core = &engine.core;
    let listed = core
        .list_dir(location.root, &location.path(""), 0, 10)?
        .entries[0]
        .token;
    past_the_tick_of(&folder.join("a.txt"), scratch.path());
    std::fs::remove_file(folder.join("a.txt"))?;
    std::fs::write(folder.join("a.txt"), b"replacement")?;
    let trashed = core.trash_entry(location.root, &location.path(""), "a.txt", Some(listed));
    assert!(
        matches!(&trashed, Err(Error::InvalidSetting(message)) if message.contains("changed")),
        "a replaced entry is not moved to the Trash: {trashed:?}"
    );
    assert_eq!(
        read(&folder.join("a.txt")),
        b"replacement",
        "the replacement is kept"
    );
    assert!(
        core.trash_entry(location.root, &location.path(""), "missing.txt", None)
            .is_err(),
        "a missing entry is not moved to the Trash"
    );
    Ok(())
}

/// A file deleted and made again under its name, so the filesystem may give
/// the new one the freed inode number (ext4 and overlayfs do at once), is
/// not the listed file: renaming or deleting it with the listed token is
/// refused. Where inode numbers are not reused this holds trivially.
#[test]
fn a_file_made_again_under_its_name_is_not_the_listed_file() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let folder = scratch.join("recycled");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("a.txt"), b"first")?;
    let (engine, location) = sharing_the_disk(&folder)?;
    let core = &engine.core;
    let listed = token_of(core, location.root, &location.path(""), "a.txt");
    assert!(listed.born.is_some(), "the listing carries a creation time");
    past_the_tick_of(&folder.join("a.txt"), scratch.path());
    std::fs::remove_file(folder.join("a.txt"))?;
    std::fs::write(folder.join("a.txt"), b"replacement")?;
    let renamed = core.rename_entry(
        location.root,
        &location.path(""),
        "a.txt",
        Some(listed),
        "b.txt",
    );
    assert!(
        renamed.as_ref().is_err_and(is_stale),
        "the file made again is not renamed as the listed one: {renamed:?}"
    );
    let deleted = core.delete_entry(location.root, &location.path(""), "a.txt", Some(listed));
    assert!(
        deleted.as_ref().is_err_and(is_stale),
        "the file made again is not deleted as the listed one: {deleted:?}"
    );
    assert_eq!(
        read(&folder.join("a.txt")),
        b"replacement",
        "the file made again is kept"
    );
    assert!(!folder.join("b.txt").exists(), "nothing was renamed");
    Ok(())
}
