//! What the files suites share: deterministic file contents, exact tree
//! comparison, jobs driven as the application drives them, paths inside a
//! test's own folders on a shared disk, and the folders an engine watches.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use vnilink::{Core, CoreEvent, EntryToken, EventSubscription, JobSnapshot, MAX_PAGE};

use crate::{holds, wait_for, Engine, Files, Pair, DEFAULT_WAIT};

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

/// `path`, a path with `/` below a root, as a job names what it copies:
/// the folder and the one name in it, or for `""` the whole root.
pub fn source(path: &str) -> (String, Vec<String>) {
    match path.rsplit_once('/') {
        _ if path.is_empty() => (String::new(), Vec::new()),
        Some((folder, name)) => (folder.to_owned(), vec![name.to_owned()]),
        None => (String::new(), vec![path.to_owned()]),
    }
}

/// The controller sends `from`, a path in its folder, to `to` in the
/// host's, as a job of `files`.
pub fn upload(pair: &Pair, files: &Files, from: &str, to: &str) -> Result<u64, vnilink::Error> {
    let (source, target) = (pair.controller.location(), pair.host.location());
    let (folder, names) = self::source(&source.path(from));
    pair.controller.core.files_service_upload(
        files.service.id,
        files.service.generation,
        source.root,
        &folder,
        &names,
        target.root,
        &target.path(to),
    )
}

/// The controller fetches `from`, a path in the host's folder, to `to` in
/// its own, as a job of `files`.
pub fn download(pair: &Pair, files: &Files, from: &str, to: &str) -> Result<u64, vnilink::Error> {
    let (source, target) = (pair.host.location(), pair.controller.location());
    let (folder, names) = self::source(&source.path(from));
    pair.controller.core.files_service_download(
        files.service.id,
        files.service.generation,
        source.root,
        &folder,
        &names,
        target.root,
        &target.path(to),
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

/// A folder of the test's own as an engine sharing its whole disk names it:
/// the disk's root (`/`, or the drive) and the path below it. The one way
/// the kit names a path to an engine: a test builds every root and path it
/// hands an engine from one, so even with the whole disk shared it writes
/// only inside its own folders. [`crate::Engine::location`] is the engine's own
/// folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub root: u64,
    pub relative: String,
}

impl Location {
    /// Where `folder`, an existing folder of the test's own, is on `core`'s
    /// shared disk, as the engine names it ([`Core::locate`]): never a
    /// disk's root itself.
    pub fn of(core: &Core, folder: &Path) -> Self {
        let (root, parent, name) = core
            .locate(folder)
            .expect("the engine is running")
            .expect("the test's folder is below a root of the engine's shared disk");
        let relative = if parent.is_empty() {
            name
        } else {
            format!("{parent}/{name}")
        };
        Self { root, relative }
    }

    /// `child`, a path with `/` below the folder, or the folder itself for
    /// `""`.
    pub fn path(&self, child: &str) -> String {
        if child.is_empty() {
            self.relative.clone()
        } else {
            format!("{}/{child}", self.relative)
        }
    }
}

/// A folder standing in for one of an engine's drives, which a test takes
/// out or replaces under a transfer, as a stick is on Windows: another
/// volume at the drive's letter is another folder at its path. While it is
/// shared the engine shares this drive alone, in place of its whole disk
/// ([`Core::share_as_drives`]); a test names paths in it through
/// [`Self::location`].
pub struct StandInDrive {
    path: std::path::PathBuf,
}

impl StandInDrive {
    /// A drive named `name` beside `engine`'s folder, holding a folder
    /// `files`, shared as the engine's only drive.
    pub fn share(engine: &Engine, name: &str) -> Self {
        let path = engine
            .folder()
            .parent()
            .expect("the engine's folder inside its scratch folder")
            .join(name);
        std::fs::create_dir_all(path.join("files")).expect("the drive's folder");
        let drive = Self { path };
        drive.read_again(engine);
        drive
    }

    /// The folder `files` on the drive, where the test's files live.
    pub fn files(&self) -> std::path::PathBuf {
        self.path.join("files")
    }

    /// [`Self::files`] as the engine names it.
    pub fn location(&self, engine: &Engine) -> Location {
        Location::of(&engine.core, &self.files())
    }

    /// Takes the drive out and puts another volume at its letter: the
    /// drive's folder moves aside, an empty one with an empty `files` takes
    /// its path, and the engine reads its drives again. Returns where the
    /// original went.
    pub fn replace(&self, engine: &Engine) -> std::path::PathBuf {
        let original = self.swap();
        self.read_again(engine);
        original
    }

    /// As [`Self::replace`], without the engine reading its drives: while it
    /// is not running.
    pub fn swap(&self) -> std::path::PathBuf {
        let original = self.path.with_extension("taken-out");
        std::fs::rename(&self.path, &original).expect("the drive taken out");
        std::fs::create_dir_all(self.path.join("files")).expect("another volume");
        original
    }

    /// Puts the original drive, moved aside by [`Self::replace`] to
    /// `original`, back at its letter, and the engine reads its drives
    /// again. The other volume moves aside; returns where it went.
    pub fn put_back(&self, engine: &Engine, original: &Path) -> std::path::PathBuf {
        let other = self.path.with_extension("other-volume");
        std::fs::rename(&self.path, &other).expect("the other volume taken out");
        std::fs::rename(original, &self.path).expect("the drive put back");
        self.read_again(engine);
        other
    }

    /// Puts another volume at the drive's letter while the drive's own
    /// folder stays where it is, as when a file on it is held open and
    /// Windows will not move it: the engine reads its drives again and
    /// finds `other`, a folder holding `files`, at the letter.
    pub fn share_other(&self, engine: &Engine, other: &Path) {
        engine
            .core
            .share_as_drives(vec![other.to_path_buf()])
            .expect("the other volume shared");
    }

    /// The engine reads its drives again and finds this one, as after a
    /// restart.
    pub fn read_again(&self, engine: &Engine) {
        engine
            .core
            .share_as_drives(vec![self.path.clone()])
            .expect("the drive shared");
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
/// lead an operation somewhere else would: a symbolic link on Unix, and on
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
