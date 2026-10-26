//! Listings: exact, sorted and paged; honest about what they leave out,
//! names that are not UTF-8 and entries past what one listing reads.

use vnilink::{EntryKind, MAX_LISTED, MAX_PAGE};
use vnilink_testkit::{traced, Engine, Scratch, TestResult};

use vnilink_testkit::files::Location;

#[test]
fn listings_are_exact_sorted_and_paged() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let root = scratch.join("root");
    std::fs::create_dir(&root)?;
    std::fs::write(root.join("beta.txt"), b"beta-bytes")?;
    std::fs::write(root.join("alpha.txt"), b"a")?;
    std::fs::write(root.join("sp ace-ß.txt"), b"unicode-bytes")?;
    std::fs::write(root.join(".hidden"), b"h")?;
    std::fs::create_dir(root.join("empty"))?;
    std::fs::create_dir_all(root.join("nested/deep"))?;
    std::fs::write(root.join("nested/deep/inner.txt"), b"inner")?;

    let engine = Engine::start("engine");
    let core = &engine.core;
    core.enable_filesystem_access()?;
    let files = Location::of(core, &root);

    let page = core.list_dir(files.root, &files.path(""), 0, 50)?;
    let names: Vec<&str> = page
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            ".hidden",
            "alpha.txt",
            "beta.txt",
            "empty",
            "nested",
            "sp ace-ß.txt"
        ],
        "a listing holds every entry, hidden ones too, sorted by name"
    );
    assert_eq!(page.total, 6, "the total counts every entry");
    assert_eq!(page.next_offset, None, "one page holds them all");
    assert!(!page.truncated, "a small folder is listed whole");

    let beta = page
        .entries
        .iter()
        .find(|e| e.name == "beta.txt")
        .ok_or("beta.txt is listed")?;
    assert_eq!(beta.kind, EntryKind::File, "a file is listed as a file");
    assert_eq!(beta.size, 10, "a file is listed with its size");
    let empty = page
        .entries
        .iter()
        .find(|e| e.name == "empty")
        .ok_or("empty is listed")?;
    assert_eq!(empty.kind, EntryKind::Dir, "a folder is listed as a folder");
    let nested = core.list_dir(files.root, &files.path("nested/deep"), 0, 50)?;
    assert_eq!(nested.entries.len(), 1, "a nested folder lists its file");
    assert_eq!(
        nested.entries[0].name, "inner.txt",
        "a nested folder lists its file"
    );
    assert!(
        core.list_dir(files.root, &files.path("empty"), 0, 50)?
            .entries
            .is_empty(),
        "an empty folder lists nothing"
    );

    // Paging walks the same order two entries at a time.
    let first = core.list_dir(files.root, &files.path(""), 0, 2)?;
    assert_eq!(
        first.next_offset,
        Some(2),
        "a page says where the next starts"
    );
    let second = core.list_dir(files.root, &files.path(""), 2, 2)?;
    assert_eq!(
        second.next_offset,
        Some(4),
        "a page says where the next starts"
    );
    let third = core.list_dir(files.root, &files.path(""), 4, 2)?;
    assert_eq!(third.next_offset, None, "the last page says it is the last");
    let all: Vec<String> = [first, second, third]
        .iter()
        .flat_map(|page| page.entries.iter().map(|e| e.name.clone()))
        .collect();
    assert_eq!(all.len(), 6, "the pages hold every entry once");
    assert_eq!(all, names, "the pages walk the listing's order");

    assert!(
        core.list_dir(files.root, &files.path(""), 0, 0).is_err(),
        "an empty page is refused"
    );
    assert!(
        core.list_dir(files.root, &files.path(""), 0, MAX_PAGE + 1)
            .is_err(),
        "a page past the largest is refused"
    );
    assert!(
        !format!("{page:?}").contains("beta"),
        "a listing's debug form names no entry"
    );
    Ok(())
}

#[test]
#[cfg(unix)]
fn listings_skip_non_utf8_names_and_count_them() -> TestResult {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let root = scratch.join("root");
    std::fs::create_dir(&root)?;
    std::fs::write(root.join("plain.txt"), b"p")?;
    let raw = OsString::from_vec(vec![0x66, 0xFF, 0x62]);

    let engine = Engine::start("engine");
    let core = &engine.core;
    core.set_file_roots(vec![root.clone()])?;
    let shared = super::only_root(core);
    // APFS refuses non-UTF8 names at creation; the listing path is still
    // exercised where the platform allows the fixture.
    if std::fs::write(root.join(&raw), b"raw").is_err() {
        let page = core.list_dir(shared, "", 0, 50)?;
        assert_eq!(page.entries.len(), 1, "the UTF-8 name is listed");
        return Ok(());
    }
    let page = core.list_dir(shared, "", 0, 50)?;
    assert_eq!(page.entries.len(), 1, "only the UTF-8 name is listed");
    assert_eq!(
        page.non_utf8_skipped, 1,
        "the name that is not UTF-8 is counted as skipped"
    );
    Ok(())
}

/// STD05: a directory larger than the list cap reports truncation honestly
/// from a single bounded pass, and an exactly full directory does not.
#[test]
fn large_directory_reports_truncated_honestly() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("filesystem");
    let big = scratch.join("big");
    let exact = scratch.join("exact");
    std::fs::create_dir_all(&big)?;
    std::fs::create_dir_all(&exact)?;
    for i in 0..=MAX_LISTED {
        std::fs::write(big.join(format!("entry-{i:06}")), [])?;
    }
    for i in 0..MAX_LISTED {
        std::fs::write(exact.join(format!("entry-{i:06}")), [])?;
    }
    let engine = Engine::start("engine");
    let core = &engine.core;
    core.enable_filesystem_access()?;
    let files = Location::of(core, scratch.path());
    let root = files.root;
    let total = u64::try_from(MAX_LISTED)?;
    let page = core.list_dir(root, &files.path("big"), 0, MAX_PAGE)?;
    assert!(
        page.truncated,
        "a listing past the cap claims complete: total={}, returned={}",
        page.total,
        page.entries.len()
    );
    assert_eq!(
        page.total, total,
        "a truncated listing counts up to the cap"
    );
    assert_eq!(
        page.entries.len(),
        usize::from(MAX_PAGE),
        "a truncated listing still fills its page"
    );
    assert_eq!(
        page.next_offset,
        Some(u64::from(MAX_PAGE)),
        "a truncated listing says where the next page starts"
    );
    let page = core.list_dir(root, &files.path("exact"), 0, MAX_PAGE)?;
    assert!(!page.truncated, "a folder exactly at the cap is whole");
    assert_eq!(page.total, total, "a folder at the cap counts every entry");
    assert_eq!(
        page.entries.len(),
        usize::from(MAX_PAGE),
        "a folder at the cap fills its page"
    );
    Ok(())
}
