// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Changes on a volume whose ids are entry positions (FAT, exFAT): a
//! listing names each entry by a fingerprint of what it saw, a file renamed
//! or deleted whose fingerprint still matches goes ahead, a replacement is
//! refused as stale, and a folder is never deleted or trashed, since no
//! fingerprint covers what it holds. Where this computer cannot mount such
//! a volume without an administrator, the test says so and proves nothing.

use std::time::{Duration, SystemTime};

use vnilink::{Core, EntryToken, Error, TokenKind};
use vnilink_testkit::files::{token_of, Location};
use vnilink_testkit::{traced, Engine, PositionalFs, Scratch, ScratchVolume, TestResult};

use super::{is_stale, read};

/// Replaces the file at `path` with another holding `bytes`, of another
/// size, as something else writing there would.
fn replace(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::remove_file(path)?;
    std::fs::write(path, bytes)
}

fn unproven(error: &Error) -> bool {
    matches!(error, Error::UnprovenEntry)
}

/// Sets `path`'s modification time to `time`.
fn modified(path: &std::path::Path, time: SystemTime) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .set_modified(time)
}

#[test]
fn positional_volumes_change_files_by_fingerprint_and_never_delete_folders() -> TestResult {
    let _trace = traced();
    for fs in [PositionalFs::Fat32, PositionalFs::ExFat] {
        let volume = match ScratchVolume::new(fs) {
            Ok(volume) => volume,
            Err(reason) => {
                vnilink_media::needs::needs(&format!("a {fs:?} volume"), Err(reason));
                continue;
            }
        };
        let folder = volume.path().join("work");
        std::fs::create_dir_all(folder.join("pack"))?;
        std::fs::write(folder.join("pack/child.txt"), b"child")?;
        std::fs::create_dir_all(folder.join("album"))?;
        for name in [
            "a.txt", "b.txt", "c.txt", "d.txt", "e.txt", "f.txt", "g.txt",
        ] {
            std::fs::write(folder.join(name), name.as_bytes())?;
        }
        let engine = Engine::start("engine");
        let core: &Core = &engine.core;
        let here = Location::of(core, &folder);
        let (root, path) = (here.root, here.path(""));
        let a = token_of(core, root, &path, "a.txt");
        assert_eq!(a.kind, TokenKind::Fingerprint, "{fs:?} lists fingerprints");

        core.rename_entry(root, &path, "a.txt", Some(a), "renamed.txt")?;
        assert_eq!(
            read(&folder.join("renamed.txt")),
            b"a.txt",
            "{fs:?}: a file whose fingerprint matches is renamed"
        );

        let b = token_of(core, root, &path, "b.txt");
        replace(&folder.join("b.txt"), b"another b, longer")?;
        assert!(
            core.rename_entry(root, &path, "b.txt", Some(b), "moved.txt")
                .is_err_and(|error| is_stale(&error)),
            "{fs:?}: a replaced file is not renamed"
        );
        assert!(
            core.delete_entry(root, &path, "b.txt", Some(b))
                .is_err_and(|error| is_stale(&error)),
            "{fs:?}: a replaced file is not deleted"
        );
        assert_eq!(
            read(&folder.join("b.txt")),
            b"another b, longer",
            "{fs:?}: the replacement stays"
        );
        let b = token_of(core, root, &path, "b.txt");
        core.delete_entry(root, &path, "b.txt", Some(b))?;
        assert!(
            !folder.join("b.txt").exists(),
            "{fs:?}: a file whose fingerprint matches is deleted"
        );

        assert!(
            core.rename_entry(root, &path, "c.txt", None, "unlisted.txt")
                .is_err_and(|error| unproven(&error)),
            "{fs:?}: a change without its listing's fingerprint proves nothing"
        );

        let pack = token_of(core, root, &path, "pack");
        for trash in [false, true] {
            let refused = if trash {
                core.trash_entry(root, &path, "pack", Some(pack))
            } else {
                core.delete_entry(root, &path, "pack", Some(pack))
            };
            assert!(
                refused.is_err_and(|error| unproven(&error)),
                "{fs:?}: a folder is never deleted (trash: {trash})"
            );
        }
        assert_eq!(
            read(&folder.join("pack/child.txt")),
            b"child",
            "{fs:?}: the folder keeps what it holds"
        );

        // Changes the volume records that keep the size: the modification
        // time alone, past FAT's 2 s; the creation time alone, the file made
        // again with the same bytes and modification time.
        let e = token_of(core, root, &path, "e.txt");
        let at = std::fs::metadata(folder.join("e.txt"))?.modified()?;
        modified(&folder.join("e.txt"), at + Duration::from_secs(10))?;
        assert!(
            core.delete_entry(root, &path, "e.txt", Some(e))
                .is_err_and(|error| is_stale(&error)),
            "{fs:?}: a file whose modification time changed is not deleted"
        );
        let f = token_of(core, root, &path, "f.txt");
        let at = std::fs::metadata(folder.join("f.txt"))?.modified()?;
        // FAT keeps creation times to 10 ms, but macOS's msdos reports them
        // to the even second: the new file is made past that tick.
        std::thread::sleep(Duration::from_millis(2_100));
        std::fs::remove_file(folder.join("f.txt"))?;
        std::fs::write(folder.join("f.txt"), b"f.txt")?;
        modified(&folder.join("f.txt"), at)?;
        assert!(
            core.rename_entry(root, &path, "f.txt", Some(f), "moved-f.txt")
                .is_err_and(|error| is_stale(&error)),
            "{fs:?}: a file made again with only another creation time is not renamed"
        );
        assert!(folder.join("f.txt").exists(), "{fs:?}: the new f stays");

        // A folder is renamed by its fingerprint, as a file is.
        let album = token_of(core, root, &path, "album");
        core.rename_entry(root, &path, "album", Some(album), "photos")?;
        assert!(
            folder.join("photos").is_dir(),
            "{fs:?}: a folder whose fingerprint matches is renamed"
        );

        // A token of the other kind, a weak one, or none proves nothing.
        let g = token_of(core, root, &path, "g.txt");
        let identity = EntryToken::identity(g.dev.unwrap_or(0), 1, None);
        assert!(
            core.rename_entry(root, &path, "g.txt", Some(identity), "x.txt")
                .is_err_and(|error| is_stale(&error)),
            "{fs:?}: an identity token proves nothing here"
        );
        for expected in [None, Some(EntryToken::NONE)] {
            assert!(
                core.delete_entry(root, &path, "g.txt", expected)
                    .is_err_and(|error| unproven(&error)),
                "{fs:?}: a delete without a fingerprint ({expected:?}) is refused"
            );
            assert!(
                core.trash_entry(root, &path, "g.txt", expected)
                    .is_err_and(|error| unproven(&error)),
                "{fs:?}: a trash without a fingerprint ({expected:?}) is refused"
            );
        }
        assert!(folder.join("g.txt").exists(), "{fs:?}: g stays");

        let c = token_of(core, root, &path, "c.txt");
        replace(&folder.join("c.txt"), b"another c, longer")?;
        assert!(
            core.trash_entry(root, &path, "c.txt", Some(c))
                .is_err_and(|error| is_stale(&error)),
            "{fs:?}: a replaced file is not trashed"
        );
        let d = token_of(core, root, &path, "d.txt");
        match core.trash_entry(root, &path, "d.txt", Some(d)) {
            Ok(()) => assert!(!folder.join("d.txt").exists(), "{fs:?}: trashed"),
            // A disk image may keep no Trash: the system refused, after
            // the fingerprint matched.
            Err(Error::Unsupported(_)) => {
                assert!(folder.join("d.txt").exists(), "{fs:?}: kept")
            }
            Err(error) => return Err(format!("{fs:?}: trash refused: {error}").into()),
        }
    }
    Ok(())
}

/// On a volume with stable ids a fingerprint proves nothing: a change
/// naming an entry by one is refused as stale.
#[test]
fn a_fingerprint_names_nothing_on_a_volume_with_stable_ids() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    std::fs::write(scratch.join("kept.txt"), b"kept")?;
    let engine = Engine::start("engine");
    let core: &Core = &engine.core;
    let here = Location::of(core, scratch.path());
    let listed = token_of(core, here.root, &here.path(""), "kept.txt");
    assert_eq!(
        listed.kind,
        TokenKind::Identity,
        "stable ids list identities"
    );
    let fingerprint = EntryToken::fingerprinted(listed.dev.unwrap_or(0), 1);
    assert!(
        core.rename_entry(
            here.root,
            &here.path(""),
            "kept.txt",
            Some(fingerprint),
            "x.txt"
        )
        .is_err_and(|error| is_stale(&error)),
        "a fingerprint renames nothing here"
    );
    assert!(
        core.delete_entry(here.root, &here.path(""), "kept.txt", Some(fingerprint))
            .is_err_and(|error| is_stale(&error)),
        "a fingerprint deletes nothing here"
    );
    assert_eq!(read(&scratch.join("kept.txt")), b"kept", "the file stays");
    Ok(())
}
