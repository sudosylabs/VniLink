//! What the files suites share: deterministic file contents, exact tree
//! comparison, jobs driven as the application drives them, a computer
//! sharing its whole disk, and the folders an engine watches.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use vnilink::{Core, CoreEvent, EntryToken, EventSubscription, JobSnapshot, MAX_PAGE};

use crate::{holds, wait_for, Engine, Pair, DEFAULT_WAIT};

/// `len` bytes that differ for each `seed` and compress poorly, as real
/// files mostly do.
pub fn payload(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    let mut bytes = Vec::with_capacity(len + 8);
    while bytes.len() < len {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        bytes.extend_from_slice(&state.wrapping_mul(0x2545_f491_4f6c_dd1d).to_le_bytes());
    }
    bytes.truncate(len);
    bytes
}

/// Writes `files` (relative path, size) under `folder`, each with its own
/// contents.
pub fn write_files(folder: &Path, files: &[(&str, usize)]) {
    for (index, (path, size)) in files.iter().enumerate() {
        let path = folder.join(path);
        std::fs::create_dir_all(path.parent().expect("a file in a folder")).expect("a folder");
        std::fs::write(path, payload(*size, index as u64 + 1)).expect("a file");
    }
}

/// One entry of a tree: a folder, or a file's size and hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Folder,
    File { size: u64, hash: String },
}

/// Every folder and file under `folder`, by relative path with `/`, so two
/// trees compare exactly: names, kinds, sizes, contents, empty folders.
pub fn tree(folder: &Path) -> BTreeMap<String, Entry> {
    fn walk(base: &Path, folder: &Path, entries: &mut BTreeMap<String, Entry>) {
        for entry in std::fs::read_dir(folder).expect("a readable folder") {
            let path = entry.expect("a folder entry").path();
            let name = path
                .strip_prefix(base)
                .expect("inside the tree")
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if path.is_dir() {
                entries.insert(name, Entry::Folder);
                walk(base, &path, entries);
            } else {
                let bytes = std::fs::read(&path).expect("a readable file");
                entries.insert(
                    name,
                    Entry::File {
                        size: bytes.len() as u64,
                        hash: blake3::hash(&bytes).to_hex().to_string(),
                    },
                );
            }
        }
    }
    let mut entries = BTreeMap::new();
    walk(folder, folder, &mut entries);
    entries
}

/// Starts both engines' background job drivers, as the application does.
pub fn drive(pair: &Pair) {
    pair.host.core.start_job_driver();
    pair.controller.core.start_job_driver();
}

/// The job `core` received from its peer, once it has one.
pub fn received(core: &Core) -> u64 {
    wait_for("the peer's job to arrive", crate::DEFAULT_WAIT, || {
        core.job_list().ok()?.first().map(|job| job.id)
    })
}

/// The job once it ended.
pub fn finished(core: &Core, job: u64, within: Duration) -> JobSnapshot {
    wait_for("the job to end", within, || {
        let snapshot = core.job_snapshot(job).ok()?;
        snapshot.state.is_terminal().then_some(snapshot)
    })
}

/// The job once it ended or stopped to ask a question. A sender mirrors its
/// receiver's question until it next asks how far the receiver is, so wait
/// for a sender with [`finished`].
pub fn settled(core: &Core, job: u64, within: Duration) -> JobSnapshot {
    wait_for("the job to end or ask", within, || {
        let snapshot = core.job_snapshot(job).ok()?;
        (snapshot.state.is_terminal() || snapshot.state == vnilink::JobState::WaitingDecision)
            .then_some(snapshot)
    })
}

/// Polls `receiver`'s job until `done` files landed, within twenty polls:
/// each poll advances a job by one step. It must not end first.
pub fn poll_until_files_done(receiver: &Core, job: u64, done: u64) {
    for _ in 0..20 {
        let snapshot = receiver.job_poll(job).expect("a poll");
        assert!(
            !snapshot.state.is_terminal(),
            "the job ended early: {snapshot:?}"
        );
        if snapshot.files_done == done {
            return;
        }
    }
    panic!("{done} files did not land within twenty polls");
}

/// Drives a job by hand, as the application's drivers would, until the
/// receiver asks a question or both sides ended: a test that must stop a
/// job at an exact point polls it this way instead of starting [`drive`].
/// Returns the sender's and the receiver's last snapshots.
pub fn drive_to_end(
    sender: &Core,
    sender_job: u64,
    receiver: &Core,
    receiver_job: u64,
    within: Duration,
) -> (JobSnapshot, JobSnapshot) {
    let mut sent = sender.job_snapshot(sender_job).expect("the sender's job");
    let mut got = receiver
        .job_snapshot(receiver_job)
        .expect("the receiver's job");
    wait_for("both sides of the job to end", within, || {
        // A poll that fails fails the job's drive: a transfer polls cleanly.
        if !got.state.is_terminal() {
            got = receiver
                .job_poll(receiver_job)
                .expect("the receiver's poll");
        }
        if !sent.state.is_terminal() {
            sent = sender.job_poll(sender_job).expect("the sender's poll");
        }
        let asking = got.state == vnilink::JobState::WaitingDecision;
        (asking || (got.state.is_terminal() && sent.state.is_terminal()))
            .then(|| (sent.clone(), got.clone()))
    })
}

/// The controller sends `from`, a path in its root, to `to` in the host's.
pub fn upload(pair: &Pair, from: &str, to: &str) -> Result<u64, vnilink::Error> {
    pair.controller.core.job_submit_upload(
        &pair.host.endpoint(),
        pair.controller.root_id(),
        from,
        pair.host.root_id(),
        to,
    )
}

/// The controller fetches `from`, a path in the host's root, to `to` in its
/// own.
pub fn download(pair: &Pair, from: &str, to: &str) -> Result<u64, vnilink::Error> {
    pair.controller.core.job_submit_download(
        &pair.host.endpoint(),
        pair.host.root_id(),
        from,
        pair.controller.root_id(),
        to,
    )
}

/// The one job `core` holds, as the peer's side of a job just submitted.
pub fn only_job(core: &Core) -> Result<u64, vnilink::Error> {
    let jobs = core.job_list()?;
    assert_eq!(jobs.len(), 1, "the peer holds exactly this job: {jobs:?}");
    Ok(jobs[0].id)
}

/// The job `core` holds that sends, or that receives.
pub fn job_on(core: &Core, sender: bool) -> Result<u64, vnilink::Error> {
    Ok(core
        .job_list()?
        .into_iter()
        .find(|job| job.sender == sender)
        .unwrap_or_else(|| panic!("a job that {}", if sender { "sends" } else { "receives" }))
        .id)
}

/// Polls `job` until `reached` holds, within `polls` polls, since each poll
/// advances a job one step: a job that needs more fails naming `what`.
/// A job that ends first fails too, unless `reached` holds for it.
pub fn poll_within(
    core: &Core,
    job: u64,
    polls: usize,
    what: &str,
    mut reached: impl FnMut(&JobSnapshot) -> bool,
) -> JobSnapshot {
    for _ in 0..polls {
        let snapshot = core.job_poll(job).expect("a poll");
        if reached(&snapshot) {
            return snapshot;
        }
        assert!(
            !snapshot.state.is_terminal(),
            "the job ended before {what}: {snapshot:?}"
        );
    }
    panic!("{what} did not happen within {polls} polls");
}

/// Every file under `folder` whose contents are `bytes`.
pub fn files_with_bytes(folder: &Path, bytes: &[u8]) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(folder) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_with_bytes(&path, bytes));
        } else if std::fs::read(&path).is_ok_and(|contents| contents == bytes) {
            found.push(path);
        }
    }
    found
}

/// Where `path` is among `core`'s file roots: the first root holding it,
/// and the path below that root with `/`.
pub fn locate(core: &Core, path: &Path) -> Option<(u64, String)> {
    core.file_roots()
        .expect("file roots")
        .into_iter()
        .find_map(|(id, root)| {
            let below = path.strip_prefix(root).ok()?;
            let below = below
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            Some((id, below))
        })
}

/// Shares `engine`'s whole disk rather than its one folder, as the
/// application does, and names `inside` (a path in its folder, the folder
/// itself for `""`) from the disk's root: the root's id and the path within
/// it.
pub fn whole_disk(
    engine: &Engine,
    inside: &str,
) -> Result<(u64, String), Box<dyn std::error::Error>> {
    engine.core.enable_filesystem_access()?;
    let folder = std::fs::canonicalize(engine.root())?.join(inside);
    Ok(locate(&engine.core, &folder).ok_or("a disk root holds the engine's folder")?)
}

/// A folder of the test's own, addressed as an engine with its whole file
/// system shared names it: a root and the path below it. Fixtures are
/// ordinary folders; only paths below the fixture are written.
#[derive(Clone)]
pub struct Location {
    pub root: u64,
    pub relative: String,
}

impl Location {
    /// Where `fixture` is among `core`'s roots.
    pub fn of(core: &Core, fixture: &Path) -> Self {
        let fixture = std::fs::canonicalize(fixture).expect("the fixture exists");
        let (root, relative) =
            locate(core, &fixture).expect("the fixture is under one of the engine's roots");
        assert!(
            !relative.is_empty(),
            "writes stay inside a temporary fixture"
        );
        Self { root, relative }
    }

    /// `child` below the fixture, or the fixture itself for `""`.
    pub fn path(&self, child: &str) -> String {
        if child.is_empty() {
            self.relative.clone()
        } else {
            format!("{}/{child}", self.relative)
        }
    }
}

/// The identity `name` in `path` is listed with, as a pane holds it; it
/// must be the native one.
pub fn token_of(core: &Core, root: u64, path: &str, name: &str) -> EntryToken {
    let token = core
        .list_dir(root, path, 0, MAX_PAGE)
        .expect("a listing")
        .entries
        .into_iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("{name} is listed"))
        .token;
    assert!(
        token.is_strong(),
        "fixture entries must carry native identity"
    );
    token
}

/// The object `path` names, where the platform says: its inode on Unix.
#[cfg(unix)]
pub fn identity(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(path).ok().map(|meta| meta.ino())
}

/// The object `path` names, where the platform says: its file index on
/// Windows, read without following a link.
#[cfg(windows)]
pub fn identity(path: &Path) -> Option<u64> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .ok()?;
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the open File keeps its handle live through the call, and the
    // output is a correctly laid out, writable BY_HANDLE_FILE_INFORMATION.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return None;
    }
    Some((u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow))
}

/// The object `path` names, where the platform says: nothing here.
#[cfg(not(any(unix, windows)))]
pub fn identity(_path: &Path) -> Option<u64> {
    None
}

/// Plants a link to the folder `target` at `link`, as something trying to
/// lead an operation out of a root would: a symbolic link on Unix, and on
/// Windows a junction, which any account can make (a symbolic link needs
/// Developer Mode or elevation) and which Windows follows as it follows a
/// symbolic link to a folder.
pub fn link_folder(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        // cmd takes only backslashes; a path joined from "a/b" carries one.
        let native = |path: &Path| path.to_string_lossy().replace('/', "\\");
        let status = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(native(link))
            .arg(native(target))
            .stdout(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other("mklink /J failed"))
        }
    }
}

/// The names a delete set aside in `folder` while it worked and did not
/// remove.
pub fn set_aside(folder: &Path) -> Vec<String> {
    std::fs::read_dir(folder)
        .expect("a readable folder")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".vnldelete."))
        .collect()
}

/// A watch may report its own start: waits until `events` has been quiet
/// for a while, and drops what came before.
pub fn settle(events: &EventSubscription) {
    wait_for("the watch to settle", DEFAULT_WAIT, || {
        events
            .recv_timeout(Duration::from_millis(300))
            .is_none()
            .then_some(())
    });
}

/// Whether `events` holds a change to `watch`, taking what it holds.
pub fn saw_change(events: &EventSubscription, watch: u64) -> bool {
    let mut seen = false;
    while let Some(event) = events.try_recv() {
        seen |= matches!(event, CoreEvent::FolderChanged { watch: changed } if changed == watch);
    }
    seen
}

/// Waits up to `within` for a change to `watch`, failing as `what`.
pub fn wait_for_change(events: &EventSubscription, watch: u64, what: &str, within: Duration) {
    wait_for(what, within, || saw_change(events, watch).then_some(()));
}

/// No change to `watch` arrives throughout `period`.
pub fn no_change(events: &EventSubscription, watch: u64, what: &str, period: Duration) {
    holds(what, period, || !saw_change(events, watch));
}
