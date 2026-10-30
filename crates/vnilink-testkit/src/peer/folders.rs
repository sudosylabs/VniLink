// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! A scenario's own folders on the agent, and the only ways a suite reaches
//! files there: it puts rendezvous files by a plain name, polls those the
//! scenario's side published, and reads what the side left under its
//! output folder. A name or path from the suite never leaves its folder:
//! no separators in a rendezvous name, no `..`, root, drive or device name
//! in an output path. Every file is opened one step at a time without
//! following a link, so a link the side plants, even between a check and
//! the read, leads nowhere, and a file with another name elsewhere (a hard
//! link) is refused. On Windows each step is opened by its full path and
//! refused when it is a reparse point; a folder on the way that the side
//! swaps for a junction between two steps could still redirect a later
//! step, which Unix rules out by opening each step from its parent's
//! handle. What the suite may put is bounded in number
//! and bytes; what a scenario accepts by name and content is its own
//! (`scenarios::launch`).

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

use super::{Listed, CHUNK};
use crate::Scratch;

/// The most bytes of one rendezvous file: they carry small JSON values.
const RENDEZVOUS_BYTES: u64 = 1024 * 1024;
/// The most rendezvous files the suite may put in one scenario.
const PUT_FILES: usize = 64;
/// The most bytes of rendezvous files the suite may put in one scenario.
const PUT_BYTES: usize = 4 * 1024 * 1024;
/// The most bytes of files one poll returns, well inside a frame once in
/// base64; the rest come with the next poll.
const POLL_BYTES: usize = 4 * 1024 * 1024;
/// The most files an output folder lists.
const LISTED: usize = 10_000;

/// A scenario's folders under the agent's temporary folder, removed with
/// it: `rendezvous` for the files the two sides exchange, `output` for its
/// evidence and `profile` for the application it starts.
pub(crate) struct Folders {
    scratch: Scratch,
    /// Rendezvous files the suite put, which polling never returns.
    put: HashSet<String>,
    /// Bytes of rendezvous files the suite put.
    put_bytes: usize,
    /// Rendezvous files already returned by a poll.
    polled: HashSet<String>,
}

impl Folders {
    /// Fresh folders tagged `tag`.
    pub(crate) fn new(tag: &str) -> Result<Self, String> {
        let scratch = Scratch::new(tag);
        for folder in ["rendezvous", "output", "profile"] {
            std::fs::create_dir(scratch.join(folder)).map_err(|error| {
                format!("the scenario's folders could not be made: {}", error.kind())
            })?;
        }
        Ok(Self {
            scratch,
            put: HashSet::new(),
            put_bytes: 0,
            polled: HashSet::new(),
        })
    }

    pub(crate) fn rendezvous(&self) -> PathBuf {
        self.scratch.join("rendezvous")
    }

    pub(crate) fn output(&self) -> PathBuf {
        self.scratch.join("output")
    }

    pub(crate) fn profile(&self) -> PathBuf {
        self.scratch.join("profile")
    }

    /// Writes the rendezvous file `name` whole, as the sides publish theirs:
    /// to a partial file renamed into place. Each name once, within the
    /// scenario's bounds.
    pub(crate) fn put(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        if !rendezvous_name(name) {
            return Err(format!("{name:?} is not a rendezvous file name"));
        }
        if bytes.len() as u64 > RENDEZVOUS_BYTES {
            return Err(format!("{name} is larger than a rendezvous file may be"));
        }
        if self.put.contains(name) {
            return Err(format!("{name} was already given"));
        }
        if self.put.len() >= PUT_FILES || self.put_bytes + bytes.len() > PUT_BYTES {
            return Err(format!(
                "a scenario takes at most {PUT_FILES} rendezvous files and {} MiB",
                PUT_BYTES / 1024 / 1024
            ));
        }
        let path = self.rendezvous().join(name);
        let partial = self.rendezvous().join(format!("{name}.partial"));
        // A new file: anything the side left at the partial name is not written through.
        let _ = std::fs::remove_file(&partial);
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)
            .and_then(|mut file| std::io::Write::write_all(&mut file, bytes))
            .and_then(|()| std::fs::rename(&partial, &path))
            .map_err(|error| format!("{name} could not be written: {}", error.kind()))?;
        self.put.insert(name.to_owned());
        self.put_bytes += bytes.len();
        Ok(())
    }

    /// The rendezvous files the side published that no poll returned yet,
    /// with their bytes, by name; at most [`POLL_BYTES`] of them, the rest
    /// on the next poll.
    pub(crate) fn poll(&mut self) -> Result<Vec<(String, Vec<u8>)>, String> {
        let rendezvous = self.rendezvous();
        let entries = std::fs::read_dir(&rendezvous).map_err(|error| {
            format!("the rendezvous folder could not be read: {}", error.kind())
        })?;
        let mut names: Vec<String> = entries
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| rendezvous_name(name))
            .filter(|name| !self.put.contains(name) && !self.polled.contains(name))
            .collect();
        names.sort();
        let mut files = Vec::with_capacity(names.len());
        let mut bytes_returned = 0;
        for name in names {
            // Only files the side wrote itself, never through a link.
            let Ok(file) = open_beneath(&rendezvous, &[name.as_str()]) else {
                continue;
            };
            let mut bytes = Vec::new();
            file.take(RENDEZVOUS_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| format!("{name} could not be read: {}", error.kind()))?;
            if bytes.len() as u64 > RENDEZVOUS_BYTES {
                return Err(format!("{name} is larger than a rendezvous file may be"));
            }
            if bytes_returned + bytes.len() > POLL_BYTES && !files.is_empty() {
                break;
            }
            bytes_returned += bytes.len();
            self.polled.insert(name.clone());
            files.push((name, bytes));
        }
        Ok(files)
    }
}

/// Whether `name` names a rendezvous file: letters, digits, `-` and `_`,
/// then `.json`, and nothing that could leave the folder or name a device.
pub(crate) fn rendezvous_name(name: &str) -> bool {
    name.strip_suffix(".json").is_some_and(|stem| {
        (1..=64).contains(&stem.len())
            && stem
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    }) && !device_name(name)
}

/// Whether Windows takes `step` for a device, whatever follows its stem:
/// `NUL`, `CON`, `PRN`, `AUX`, `COM1`..`COM9`, `LPT1`..`LPT9`.
fn device_name(step: &str) -> bool {
    let stem = step
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}

/// Every file under `output`, by its path there with `/` between folders,
/// sorted; links are neither listed nor followed.
pub(crate) fn list(output: &Path) -> Result<Vec<Listed>, String> {
    let mut files = Vec::new();
    let mut folders = vec![(output.to_path_buf(), String::new())];
    while let Some((folder, prefix)) = folders.pop() {
        let entries = std::fs::read_dir(&folder)
            .map_err(|error| format!("the output folder could not be read: {}", error.kind()))?;
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            // The entry's own kind: a link is a link, not what it names.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = format!("{prefix}{name}");
            if kind.is_dir() {
                folders.push((entry.path(), format!("{path}/")));
            } else if kind.is_file() {
                let size = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
                files.push(Listed { path, size });
                if files.len() > LISTED {
                    return Err(format!("the output folder holds more than {LISTED} files"));
                }
            }
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Up to [`CHUNK`] bytes of the file `path` under `output` from `offset`,
/// and whether they are its last.
pub(crate) fn read_chunk(
    output: &Path,
    path: &str,
    offset: u64,
) -> Result<(Vec<u8>, bool), String> {
    let refused = || format!("{path:?} is not a file of the scenario's output");
    let steps = steps(path).ok_or_else(refused)?;
    let mut opened = open_beneath(output, &steps).map_err(|_| refused())?;
    let failed = |error: std::io::Error| format!("{path} could not be read: {}", error.kind());
    let size = opened.metadata().map_err(failed)?.len();
    opened
        .seek(SeekFrom::Start(offset.min(size)))
        .map_err(failed)?;
    let mut bytes = Vec::new();
    opened
        .take(CHUNK as u64)
        .read_to_end(&mut bytes)
        .map_err(failed)?;
    let end = offset.saturating_add(bytes.len() as u64) >= size;
    Ok((bytes, end))
}

/// The plain names `path` joins with `/`; `None` for any other path: empty
/// steps, `.`, `..`, roots, drives, `\`, device names, or a step Windows
/// would trim.
pub(crate) fn steps(path: &str) -> Option<Vec<&str>> {
    if path.is_empty() || path.contains('\\') || path.contains(':') {
        return None;
    }
    path.split('/')
        .map(|step| {
            let mut components = Path::new(step).components();
            let plain = matches!(
                (components.next(), components.next()),
                (Some(Component::Normal(name)), None) if name == step
            );
            (plain && !device_name(step) && !step.ends_with('.') && !step.ends_with(' '))
                .then_some(step)
        })
        .collect()
}

/// The regular file `steps` names under `root`, opened one step at a time
/// from a handle on the step before, following no link: a link anywhere on
/// the way, or a file that is not a regular one, is refused.
#[cfg(unix)]
fn open_beneath(root: &Path, steps: &[&str]) -> std::io::Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let refused = || std::io::Error::from(std::io::ErrorKind::InvalidInput);
    let mut current = File::open(root)?;
    for (index, step) in steps.iter().enumerate() {
        let name = std::ffi::CString::new(*step).map_err(|_| refused())?;
        let last = index + 1 == steps.len();
        // Non-blocking, so a pipe planted in place of a file cannot hold
        // the agent; reading a regular file ignores it.
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | if last {
                libc::O_NONBLOCK
            } else {
                libc::O_DIRECTORY
            };
        // SAFETY: `current` is an open descriptor for the call's duration
        // and `name` a NUL-terminated string; the call has no other effect.
        let descriptor = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `openat` returned a new descriptor that nothing else owns.
        current = unsafe { File::from_raw_fd(descriptor) };
    }
    let metadata = current.metadata()?;
    // A hard link would read a file the side does not own under its name.
    if !metadata.is_file() || std::os::unix::fs::MetadataExt::nlink(&metadata) != 1 {
        return Err(refused());
    }
    Ok(current)
}

/// As the Unix form; Windows opens each step's reparse point itself
/// rather than what it points to, and refuses any.
#[cfg(windows)]
fn open_beneath(root: &Path, steps: &[&str]) -> std::io::Result<File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};

    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    let refused = || std::io::Error::from(std::io::ErrorKind::InvalidInput);
    let mut path = root.to_path_buf();
    let mut current = None;
    for step in steps {
        path.push(step);
        let opened = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(&path)?;
        if opened.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(refused());
        }
        current = Some(opened);
    }
    let file = current.ok_or_else(refused)?;
    if !file.metadata()?.is_file() || links(&file)? != 1 {
        return Err(refused());
    }
    Ok(file)
}

/// How many names the open `file` has.
#[cfg(windows)]
fn links(file: &File) -> std::io::Result<u32> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    // SAFETY: an all-zero BY_HANDLE_FILE_INFORMATION is a valid value of
    // that plain C struct, filled in by the call below.
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is open for the call's duration and `information`
    // is a valid place for the call to write.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(information.nNumberOfLinks)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rendezvous name is a plain `.json` file name and nothing that
    /// could reach another folder or a device.
    #[test]
    fn a_rendezvous_name_is_a_plain_json_file_name() {
        for name in [
            "host.json",
            "read-done.json",
            "viewer_cpu.json",
            "console.json",
        ] {
            assert!(rendezvous_name(name), "{name} is a rendezvous name");
        }
        for name in [
            ".json",
            "host",
            "host.txt",
            "../host.json",
            "a/b.json",
            "a\\b.json",
            "host.json.partial",
            "ünïcode.json",
            "NUL.json",
            "con.json",
            "COM1.json",
            "lpt9.json",
            &format!("{}.json", "a".repeat(65)),
        ] {
            assert!(!rendezvous_name(name), "{name:?} is refused");
        }
    }

    /// What the suite puts is written whole, once per name, within the
    /// scenario's bounds, and never polled back; what the side publishes is
    /// polled once.
    #[test]
    fn a_poll_returns_what_the_side_published_once_and_never_what_was_put() -> crate::TestResult {
        let mut folders = Folders::new("folders")?;
        folders.put("viewer.json", b"{\"token\":1}")?;
        assert_eq!(
            std::fs::read(folders.rendezvous().join("viewer.json"))?,
            b"{\"token\":1}",
            "the side reads what was put"
        );
        assert!(
            folders.put("viewer.json", b"{}").is_err(),
            "a name is given once"
        );
        assert!(
            folders.put("../escape.json", b"{}").is_err(),
            "a name that leaves the folder is refused"
        );
        std::fs::write(folders.rendezvous().join("host.json"), b"{}")?;
        std::fs::write(folders.rendezvous().join("host-ready.json.partial"), b"{")?;
        assert_eq!(
            folders.poll()?,
            vec![("host.json".to_owned(), b"{}".to_vec())],
            "only the side's whole files"
        );
        assert_eq!(folders.poll()?, Vec::new(), "each file once");
        Ok(())
    }

    /// The suite puts at most so many files and bytes in one scenario.
    #[test]
    fn puts_are_bounded_in_number_and_bytes() -> crate::TestResult {
        let mut folders = Folders::new("folders")?;
        for index in 0..PUT_FILES {
            folders.put(&format!("f{index}.json"), b"{}")?;
        }
        assert!(
            folders.put("one-more.json", b"{}").is_err(),
            "no more than {PUT_FILES} files"
        );
        let mut folders = Folders::new("folders")?;
        let large = vec![b' '; RENDEZVOUS_BYTES as usize];
        for index in 0..PUT_BYTES / large.len() {
            folders.put(&format!("large{index}.json"), &large)?;
        }
        assert!(
            folders.put("over.json", b"{}").is_err(),
            "no more than {PUT_BYTES} bytes"
        );
        Ok(())
    }

    /// The output folder lists its files by relative path, and only those
    /// are read: a path out of it, through a link or naming a device, is
    /// refused.
    #[test]
    fn only_files_under_the_output_folder_are_read() -> crate::TestResult {
        let folders = Folders::new("folders")?;
        let output = folders.output();
        std::fs::create_dir_all(output.join("logs"))?;
        std::fs::write(output.join("summary.md"), b"# run")?;
        std::fs::write(output.join("logs/host.log"), vec![b'x'; CHUNK + 10])?;
        std::fs::write(folders.profile().join("secret"), b"profile")?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(folders.profile(), output.join("linked"))?;
        assert_eq!(
            list(&output)?,
            vec![
                Listed {
                    path: "logs/host.log".to_owned(),
                    size: (CHUNK + 10) as u64
                },
                Listed {
                    path: "summary.md".to_owned(),
                    size: 5
                },
            ],
            "files by relative path; no link"
        );
        assert_eq!(
            read_chunk(&output, "summary.md", 0)?,
            (b"# run".to_vec(), true)
        );
        let (first, end) = read_chunk(&output, "logs/host.log", 0)?;
        assert_eq!((first.len(), end), (CHUNK, false), "a chunk at a time");
        let (rest, end) = read_chunk(&output, "logs/host.log", CHUNK as u64)?;
        assert_eq!((rest.len(), end), (10, true), "then the rest");
        for path in [
            "../profile/secret",
            "linked/secret",
            "/etc/hosts",
            "logs/../summary.md",
            "logs",
            "",
            "C:\\Windows",
            "./summary.md",
            "NUL",
            "logs/con.log",
            "summary.md.",
        ] {
            assert!(
                read_chunk(&output, path, 0).is_err(),
                "{path:?} is not read"
            );
        }
        Ok(())
    }

    /// A link the side plants is never followed: not by a poll, not by a
    /// fetch, whether it stands for a file or for a folder on the way.
    #[cfg(unix)]
    #[test]
    fn a_link_the_side_plants_is_not_followed() -> crate::TestResult {
        let mut folders = Folders::new("folders")?;
        let outside = Scratch::new("outside");
        std::fs::write(outside.join("secret.json"), b"{\"secret\":1}")?;
        std::os::unix::fs::symlink(
            outside.join("secret.json"),
            folders.rendezvous().join("host.json"),
        )?;
        assert_eq!(folders.poll()?, Vec::new(), "a poll does not follow a link");
        let output = folders.output();
        std::os::unix::fs::symlink(outside.join("secret.json"), output.join("leak.log"))?;
        std::os::unix::fs::symlink(outside.path(), output.join("logs"))?;
        for path in ["leak.log", "logs/secret.json"] {
            assert!(
                read_chunk(&output, path, 0).is_err(),
                "a fetch does not follow {path}"
            );
        }
        // A hard link to a file outside is no file of the side's.
        std::fs::hard_link(outside.join("secret.json"), output.join("hard.log"))?;
        assert!(
            read_chunk(&output, "hard.log", 0).is_err(),
            "a hard link is refused"
        );
        std::fs::hard_link(
            outside.join("secret.json"),
            folders.rendezvous().join("viewer.json"),
        )?;
        assert_eq!(
            folders.poll()?,
            Vec::new(),
            "a poll refuses a hard link too"
        );
        // A pipe in place of a file does not hold the agent.
        let pipe = std::ffi::CString::new(
            output
                .join("pipe.log")
                .into_os_string()
                .into_encoded_bytes(),
        )?;
        // SAFETY: `pipe` is a NUL-terminated path; the call only makes a FIFO there.
        assert_eq!(unsafe { libc::mkfifo(pipe.as_ptr(), 0o600) }, 0, "a FIFO");
        assert!(
            read_chunk(&output, "pipe.log", 0).is_err(),
            "a pipe is no file"
        );
        Ok(())
    }
}
