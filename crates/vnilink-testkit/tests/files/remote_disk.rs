//! The host's disk through the Files service: browsing, making, renaming and
//! deleting, under the grant, the generation and the path rules. Every
//! request names a path inside the host engine's own folder.

use expect_test::expect;
use std::path::Path;

use vnilink::{Error, Grants};
use vnilink_testkit::files::{download, identity, set_aside};
use vnilink_testkit::{traced, Pair, TestResult};

/// Replaces the file at `path` with another object holding `bytes`. The
/// replacement is made before the original goes, so a recycled identity
/// cannot make the two compare the same.
fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let aside = path.with_extension("tmp");
    std::fs::write(&aside, bytes)?;
    std::fs::remove_file(path)?;
    std::fs::rename(aside, path)
}

/// A host shares its disk again only while no session is active.
#[test]
fn sharing_the_disk_is_refused_during_an_active_session() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_files();
    let before = pair.host.core.file_roots()?;
    // The controller's Files hold their connection for the whole check.
    let _files = pair.open_files();
    let refused = pair.host.core.enable_filesystem_access();
    assert!(
        matches!(refused, Err(Error::Busy)),
        "the disk is shared again only without a session: {refused:?}"
    );
    assert_eq!(
        pair.host.core.file_roots()?,
        before,
        "the refusal left the shared roots as they were"
    );
    Ok(())
}

/// The controller browses the host's folder, makes a folder there and
/// renames a file, and the host's disk changes as it asked: a rename is the
/// same object with the same bytes, and the next listing shows it.
#[test]
fn remote_browse_mkdir_and_rename_change_the_host_disk() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let root = pair.host.folder();
    std::fs::write(root.join("report.txt"), b"report-bytes")?;
    let here = pair.host.location();
    let (disk, folder) = (here.root, here.path(""));
    let files = pair.open_files();

    let page = files.pipe.list(disk, &folder, 0, 50)?;
    assert_eq!(page.total, 1, "the host's folder holds one entry");
    assert_eq!(page.entries[0].name, "report.txt", "it is the host's file");
    let token = page.entries[0].token;

    files.pipe.mkdir(disk, &folder, "archive")?;
    assert!(
        root.join("archive").is_dir(),
        "a remote mkdir lands on the host's disk"
    );

    let before = identity(&root.join("report.txt"));
    files
        .pipe
        .rename(disk, &folder, "report.txt", Some(token), "final.txt")?;
    assert!(
        !root.join("report.txt").exists(),
        "a renamed file leaves its old name"
    );
    assert_eq!(
        std::fs::read(root.join("final.txt"))?,
        b"report-bytes",
        "a renamed file keeps its bytes"
    );
    assert_eq!(
        identity(&root.join("final.txt")),
        before,
        "a remote rename is a true rename of the same object"
    );

    let again = files.pipe.list(disk, &folder, 0, 50)?;
    let names: Vec<&str> = again.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        ["archive", "final.txt"],
        "the remote pane refreshes to the new state"
    );
    trace.wait_for_records("file request answered", 4);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          files: file request answered body_bytes=90 operation=list status=0
          files: file request answered body_bytes=0 operation=create folder status=0
          files: file request answered body_bytes=0 operation=rename status=0
          files: file request answered body_bytes=149 operation=list status=0
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// A job for a stale generation of the service, a request whose path or
/// name steps out of its folder, and any after the host revoked the
/// controller never touch the host's disk.
#[test]
fn remote_requests_enforce_generation_grant_and_scope() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let root = pair.host.folder();
    std::fs::write(root.join("kept.txt"), b"kept-bytes")?;
    let here = pair.host.location();
    let (disk, folder) = (here.root, here.path(""));
    let files = pair.open_files();
    let kept = || std::fs::read(root.join("kept.txt")).ok();

    // The pipe stamps its own generation; a job names the service's, and
    // a stale one submits nothing.
    let mine = pair.controller.location();
    let stale = pair.controller.core.files_service_download(
        files.service.id,
        files.service.generation + 1,
        disk,
        &folder,
        &["kept.txt".to_owned()],
        mine.root,
        &mine.path("got"),
    );
    assert!(
        matches!(stale, Err(Error::StaleSession)),
        "a stale generation submits no job: {stale:?}"
    );
    assert!(
        pair.host.core.job_list()?.is_empty(),
        "the host was offered no job"
    );
    assert_eq!(
        kept().as_deref(),
        Some(&b"kept-bytes"[..]),
        "the file stayed"
    );
    assert!(set_aside(&root).is_empty(), "nothing was set aside");

    assert!(
        files
            .pipe
            .list(disk, &format!("{folder}/.."), 0, 50)
            .is_err(),
        "a listing that steps out of its folder is refused"
    );
    assert!(
        files
            .pipe
            .rename(disk, &folder, "kept.txt", None, "../out.txt")
            .is_err(),
        "a rename out of its folder is refused"
    );
    assert!(
        files
            .pipe
            .delete(disk, &folder, "../kept.txt", None, false)
            .is_err(),
        "a delete out of its folder is refused"
    );
    assert!(
        !root.with_file_name("out.txt").exists(),
        "nothing landed outside the folder"
    );
    assert_eq!(
        kept().as_deref(),
        Some(&b"kept-bytes"[..]),
        "the file stayed"
    );

    // Revocation stops the connection, and with it any filesystem work.
    pair.host.core.revoke(&pair.controller.endpoint())?;
    assert!(
        files.pipe.list(disk, &folder, 0, 50).is_err(),
        "a revoked controller lists nothing"
    );
    assert_eq!(
        kept().as_deref(),
        Some(&b"kept-bytes"[..]),
        "the file stayed"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          files: file request answered body_bytes=0 operation=list status=4
          files: file request answered body_bytes=0 operation=rename status=4
          files: file request answered body_bytes=0 operation=delete status=4
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// A rename with the token of a file since replaced is refused and leaves
/// the replacement; a rename onto a name made after the listing does not
/// overwrite it.
#[test]
fn remote_rename_conflict_and_stale_token_hold() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let root = pair.host.folder();
    std::fs::write(root.join("a.txt"), b"a-bytes")?;
    let here = pair.host.location();
    let (disk, folder) = (here.root, here.path(""));
    let files = pair.open_files();

    let page = files.pipe.list(disk, &folder, 0, 50)?;
    let stale = page.entries[0].token;
    replace(&root.join("a.txt"), b"replacement-bytes")?;
    let refused = files
        .pipe
        .rename(disk, &folder, "a.txt", Some(stale), "b.txt");
    assert!(
        refused.is_err(),
        "a stale token renames no replacement: {refused:?}"
    );
    assert!(!root.join("b.txt").exists(), "nothing moved");
    assert_eq!(
        std::fs::read(root.join("a.txt"))?,
        b"replacement-bytes",
        "the replacement stayed"
    );

    std::fs::write(root.join("b.txt"), b"b-bytes")?;
    let fresh = files.pipe.list(disk, &folder, 0, 50)?;
    let current = fresh
        .entries
        .iter()
        .find(|entry| entry.name == "a.txt")
        .ok_or("a.txt is listed")?
        .token;
    let conflict = files
        .pipe
        .rename(disk, &folder, "a.txt", Some(current), "b.txt");
    assert!(
        matches!(conflict, Err(Error::FileExists)),
        "a rename never overwrites: {conflict:?}"
    );
    assert_eq!(
        std::fs::read(root.join("a.txt"))?,
        b"replacement-bytes",
        "the source stayed"
    );
    assert_eq!(
        std::fs::read(root.join("b.txt"))?,
        b"b-bytes",
        "the destination stayed"
    );
    trace.wait_for_records("file request answered", 4);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          files: file request answered body_bytes=85 operation=list status=0
          files: file request answered body_bytes=0 operation=rename status=4
          files: file request answered body_bytes=143 operation=list status=0
          files: file request answered body_bytes=0 operation=rename status=1
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// Files need the transfer grant alone: the host shares neither view nor
/// control, and the controller has no decoder.
#[test]
fn files_need_transfer_not_control_or_decoder() -> TestResult {
    let _trace = traced();
    let none = Grants {
        view: false,
        control: false,
        clipboard: false,
        transfer: false,
    };
    let pair = Pair::new(
        Grants {
            transfer: true,
            ..none
        },
        none,
    );
    std::fs::write(pair.host.folder().join("shared.txt"), b"shared-bytes")?;
    // Files need no decoder.
    pair.controller
        .core
        .set_video_codecs(vnilink::VideoCodecCaps::default())?;
    let files = pair.controller.files_on(&pair.host)?;
    let page = files.pipe.list(
        pair.host.location().root,
        &pair.host.location().path(""),
        0,
        50,
    )?;
    assert!(
        page.entries.iter().any(|entry| entry.name == "shared.txt"),
        "Files on the transfer grant alone browse the host"
    );
    Ok(())
}

/// The host refuses to delete a file an active transfer serves, and
/// deletes it once the transfer ended.
#[test]
fn delete_during_active_transfer_is_refused() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let host = &pair.host.core;
    let root = pair.host.folder();
    let here = pair.host.location();
    std::fs::write(root.join("a.txt"), b"transfer-bytes")?;
    let files = pair.open_files();
    let token = host
        .list_dir(here.root, &here.path(""), 0, 50)?
        .entries
        .into_iter()
        .find(|entry| entry.name == "a.txt")
        .ok_or("a.txt is listed")?
        .token;
    assert!(token.is_strong(), "a listed file carries its own identity");
    let job = download(&pair, &files, "a.txt", "")?;
    let serving = host.job_list()?;
    assert_eq!(serving.len(), 1, "the host serves one job");
    assert!(
        !serving[0].state.is_terminal(),
        "the host's job is active: {:?}",
        serving[0].state
    );

    let blocked = host.delete_entry(here.root, &here.path(""), "a.txt", Some(token));
    assert!(
        matches!(
            blocked,
            Err(Error::InvalidSetting(
                "the entry is used by an active transfer"
            ))
        ),
        "a file being served is not deleted: {blocked:?}"
    );
    assert_eq!(
        std::fs::read(root.join("a.txt"))?,
        b"transfer-bytes",
        "the served file stayed"
    );
    assert!(set_aside(&root).is_empty(), "nothing was set aside");

    pair.controller.core.job_cancel(job)?;
    let ended = host.job_list()?[0].state;
    assert!(
        ended.is_terminal(),
        "a cancel ends the host's job too: {ended:?}"
    );
    host.delete_entry(here.root, &here.path(""), "a.txt", Some(token))?;
    assert!(
        !root.join("a.txt").exists(),
        "a file no transfer serves is deleted"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state done_bytes=0 files_done=0 from=transferring job=1 sender=false to=cancelled
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state done_bytes=0 files_done=0 from=serving job=1 sender=true to=cancelled
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// A remote delete with the token of a file since replaced is refused and
/// keeps the replacement; with current tokens it removes a file and a
/// folder tree, leaving nothing aside.
#[test]
fn remote_delete_removes_the_host_entry_and_keeps_a_replacement() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let root = pair.host.folder();
    std::fs::create_dir_all(root.join("pack"))?;
    std::fs::write(root.join("pack/child.txt"), b"child-bytes")?;
    std::fs::write(root.join("report.txt"), b"report-bytes")?;
    let here = pair.host.location();
    let (disk, folder) = (here.root, here.path(""));
    let files = pair.open_files();
    let token_of = |page: &vnilink::DirPage, name: &str| {
        page.entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.token)
            .ok_or(format!("{name} is listed"))
    };

    let page = files.pipe.list(disk, &folder, 0, 50)?;
    let report = token_of(&page, "report.txt")?;
    let pack = token_of(&page, "pack")?;
    replace(&root.join("report.txt"), b"replacement-bytes")?;
    let refused = files
        .pipe
        .delete(disk, &folder, "report.txt", Some(report), false);
    assert!(
        refused.is_err(),
        "a stale token deletes no replacement: {refused:?}"
    );
    assert_eq!(
        std::fs::read(root.join("report.txt"))?,
        b"replacement-bytes",
        "the replacement stayed"
    );
    assert!(set_aside(&root).is_empty(), "nothing was set aside");

    let fresh = files.pipe.list(disk, &folder, 0, 50)?;
    let current = token_of(&fresh, "report.txt")?;
    files
        .pipe
        .delete(disk, &folder, "report.txt", Some(current), false)?;
    files
        .pipe
        .delete(disk, &folder, "pack", Some(pack), false)?;
    assert!(!root.join("report.txt").exists(), "the file was deleted");
    assert!(!root.join("pack").exists(), "the folder tree was deleted");
    assert!(set_aside(&root).is_empty(), "nothing was left aside");
    trace.wait_for_records("file request answered", 5);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          files: file request answered body_bytes=147 operation=list status=0
          files: file request answered body_bytes=0 operation=delete status=4
          files: file request answered body_bytes=147 operation=list status=0
          files: file request answered body_bytes=0 operation=delete status=0
          files: file request answered body_bytes=0 operation=delete status=0
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}
