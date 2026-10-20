//! Job records on disk: one append-only journal per job.
//!
//! A journal starts with the whole record, manifest included, and grows
//! by what changed since: the record's progress without its manifest, or
//! one file that landed. A save writes only the jobs that changed, so a
//! job of many files costs a short line per file instead of a rewrite of
//! every record, and a save that changes nothing writes nothing and tells
//! no one. Once a journal has grown past twice its first line, the next
//! change rewrites it as one record again.
//!
//! Journals live in `jobs/<id>.jsonl` beside the blob store. Nothing is
//! synced: a crash loses at most the last lines, and a torn last line is
//! ignored when the journals are read again.

use super::*;

use serde::{Deserialize, Serialize};

use super::jobs::JobRecord;

pub(super) const JOURNAL_DIR: &str = "jobs";
const JOURNAL_SUFFIX: &str = ".jsonl";
const TMP_SUFFIX: &str = ".jsonl.tmp";
const SAVE_FAILED: &str = "the job record could not be saved";
/// Lines appended past the first one before a rewrite is due, at least.
const MIN_COMPACT_BYTES: u64 = 64 * 1024;

/// One journal line.
#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum Line<'a> {
    /// The whole record. Always the first line.
    Record(&'a JobRecord),
    /// A file of the manifest landed under this name.
    Landed(usize, &'a str),
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ReadLine {
    Record(Box<JobRecord>),
    Progress(Box<JobRecord>),
    Landed(usize, String),
}

/// What the journal of one job holds, as last written.
struct Written {
    /// The progress line the journal ends with, in effect.
    progress: String,
    has_manifest: bool,
    /// Bytes of the first line, and of everything after it.
    record_bytes: u64,
    appended_bytes: u64,
}

/// The journals of every job, as last written. Its lock also orders
/// writers: a save gathers and writes under it.
#[derive(Default)]
pub(super) struct JobJournal {
    written: HashMap<u64, Written>,
}

/// What one save does to one job's journal.
enum Write {
    /// Writes the journal afresh from this first line, which holds this
    /// many landed files the journal had not heard of.
    Record(String, usize),
    /// Appends these lines; the count of landed files they carry.
    Append(String, usize),
    Remove,
}

/// Saves the jobs that changed since they were last saved and, when any
/// did, tells watchers and the driver. Without a blob directory nothing
/// reaches disk, but changes are still told.
pub(super) fn save_jobs(shared: &Shared) -> Result<(), Error> {
    let mut journal = lock(&shared.job_journal);
    let (writes, progress) = {
        let mut jobs = lock(&shared.jobs);
        let mut writes = Vec::new();
        let mut progress = HashMap::new();
        for (id, record) in jobs.iter_mut() {
            let now = record.progress_line();
            let written = journal.written.get(id);
            let compact = written.is_some_and(|written| {
                written.appended_bytes > (2 * written.record_bytes).max(MIN_COMPACT_BYTES)
            });
            let write = match written {
                Some(written) if written.has_manifest == record.has_manifest() && !compact => {
                    let landed = record.unsaved_landed();
                    if written.progress == now && landed.is_empty() {
                        continue;
                    }
                    let mut lines = String::new();
                    for (index, name) in landed {
                        push_line(&mut lines, &Line::Landed(*index, name))?;
                    }
                    if written.progress != now {
                        // The record as it is now, already in JSON.
                        lines.push_str("{\"progress\":");
                        lines.push_str(&now);
                        lines.push_str("}\n");
                    }
                    Write::Append(lines, landed.len())
                }
                _ => {
                    let mut line = String::new();
                    push_line(&mut line, &Line::Record(record))?;
                    Write::Record(line, record.unsaved_landed().len())
                }
            };
            progress.insert(*id, (now, record.has_manifest()));
            writes.push((*id, write));
        }
        for id in journal.written.keys() {
            if !jobs.contains_key(id) {
                writes.push((*id, Write::Remove));
            }
        }
        (writes, progress)
    };
    if writes.is_empty() {
        return Ok(());
    }
    // Whatever lands on disk below changed: watchers hear of it even when
    // the disk refuses it.
    super::jobs::jobs_changed(shared);
    let dir = match lock(&shared.blob_dir).clone() {
        Some(dir) => Some(dir.join(JOURNAL_DIR)),
        None => None,
    };
    if let Some(dir) = &dir {
        std::fs::create_dir_all(dir).map_err(|_| Error::InvalidSetting(SAVE_FAILED))?;
    }
    for (id, write) in writes {
        if let Some(dir) = &dir {
            write_journal(dir, id, &write)?;
        }
        match write {
            Write::Remove => {
                journal.written.remove(&id);
            }
            Write::Record(line, landed) => {
                let (progress, has_manifest) = progress[&id].clone();
                // Files landing while it was written are still unsaved.
                if let Some(record) = lock(&shared.jobs).get_mut(&id) {
                    record.saved_landed(landed);
                }
                journal.written.insert(
                    id,
                    Written {
                        progress,
                        has_manifest,
                        record_bytes: line.len() as u64,
                        appended_bytes: 0,
                    },
                );
            }
            Write::Append(lines, landed) => {
                let (progress, _) = progress[&id].clone();
                if let Some(record) = lock(&shared.jobs).get_mut(&id) {
                    record.saved_landed(landed);
                }
                if let Some(written) = journal.written.get_mut(&id) {
                    written.progress = progress;
                    written.appended_bytes += lines.len() as u64;
                }
            }
        }
    }
    Ok(())
}

fn push_line(out: &mut String, line: &Line<'_>) -> Result<(), Error> {
    let text = serde_json::to_string(line).map_err(|_| Error::InvalidSetting(SAVE_FAILED))?;
    out.push_str(&text);
    out.push('\n');
    Ok(())
}

fn journal_path(dir: &std::path::Path, id: u64) -> std::path::PathBuf {
    dir.join(format!("{id}{JOURNAL_SUFFIX}"))
}

fn write_journal(dir: &std::path::Path, id: u64, write: &Write) -> Result<(), Error> {
    let path = journal_path(dir, id);
    let failed = |_| Error::InvalidSetting(SAVE_FAILED);
    match write {
        Write::Record(line, _) => {
            let tmp = dir.join(format!("{id}{TMP_SUFFIX}"));
            std::fs::write(&tmp, line).map_err(failed)?;
            std::fs::rename(&tmp, &path).map_err(failed)
        }
        Write::Append(lines, _) => {
            use std::io::Write as _;
            std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .and_then(|mut file| file.write_all(lines.as_bytes()))
                .map_err(failed)
        }
        Write::Remove => match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(failed(error)),
            _ => Ok(()),
        },
    }
}

/// Reads every journal under `dir`. `None` when there is no journal
/// directory at all; otherwise the records and whether every journal
/// started with a readable record, which is what allows cleaning up
/// after jobs that no journal names.
pub(super) fn read_journals(dir: &std::path::Path) -> Option<(Vec<JobRecord>, bool)> {
    let entries = std::fs::read_dir(dir.join(JOURNAL_DIR)).ok()?;
    let mut records = Vec::new();
    let mut clean = true;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.ends_with(TMP_SUFFIX) {
            // An interrupted rewrite; the journal it would replace stands.
            let _ = std::fs::remove_file(entry.path());
            continue;
        }
        let Some(id) = name
            .strip_suffix(JOURNAL_SUFFIX)
            .and_then(|id| id.parse::<u64>().ok())
        else {
            continue;
        };
        match std::fs::read_to_string(entry.path())
            .ok()
            .and_then(|text| replay(&text))
        {
            Some(record) if record.id == id => records.push(record),
            _ => clean = false,
        }
    }
    Some((records, clean))
}

/// The record a journal ends with. A line that does not read, such as
/// one torn by a crash, ends the journal there.
fn replay(text: &str) -> Option<JobRecord> {
    let mut lines = text.lines();
    let ReadLine::Record(record) = serde_json::from_str(lines.next()?).ok()? else {
        return None;
    };
    let mut record = *record;
    for line in lines {
        match serde_json::from_str(line) {
            Ok(ReadLine::Progress(progress)) => record.take_progress(*progress),
            Ok(ReadLine::Landed(index, name)) => record.land_saved(index, name),
            Ok(ReadLine::Record(_)) | Err(_) => break,
        }
    }
    Some(record)
}

impl JobJournal {
    /// Forgets what was written: the journals of another blob directory
    /// are about to be read, and each job's next save writes it afresh.
    pub(super) fn reset(&mut self) {
        self.written.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn core_with_blobs() -> (Core, Scratch) {
        let scratch = Scratch(std::env::temp_dir().join(format!(
            "vnilink-journal-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        )));
        std::fs::create_dir_all(&scratch.0).unwrap();
        let core = Core::boot(
            CoreConfig {
                listen: Listen::Loopback,
                ..CoreConfig::default()
            },
            iroh::RelayMode::Disabled,
        )
        .unwrap();
        core.set_blob_directory(scratch.0.join("blobs")).unwrap();
        (core, scratch)
    }

    fn journal_of(scratch: &Scratch, id: u64) -> std::path::PathBuf {
        scratch
            .0
            .join("blobs")
            .join(JOURNAL_DIR)
            .join(format!("{id}.jsonl"))
    }

    /// A job of many files grows its journal by a short line per file
    /// instead of rewriting its manifest, a save that changes nothing
    /// writes nothing and tells no one, and the journal reads back as the
    /// record it was, landed files included.
    #[test]
    fn journals_hold_what_changed_and_read_back_as_the_record() {
        const FILES: usize = 2000;
        let (core, scratch) = core_with_blobs();
        let id = 7;
        lock(&core.shared.jobs).insert(id, JobRecord::test_receiver_of(id, FILES));
        save_jobs(&core.shared).unwrap();
        let path = journal_of(&scratch, id);
        let first = std::fs::metadata(&path).unwrap().len();
        assert!(
            first > 100 * FILES as u64,
            "the first line holds the manifest"
        );

        let revision = core.job_revision();
        save_jobs(&core.shared).unwrap();
        assert_eq!(
            core.job_revision(),
            revision,
            "an unchanged save told watchers"
        );
        assert_eq!(std::fs::metadata(&path).unwrap().len(), first);

        let mut largest = 0;
        for _ in 0..FILES {
            lock(&core.shared.jobs)
                .get_mut(&id)
                .unwrap()
                .test_land_next();
            save_jobs(&core.shared).unwrap();
            largest = largest.max(std::fs::metadata(&path).unwrap().len());
        }
        // Rewritten once it outgrows its first line twice over, never more.
        assert!(
            largest < 3 * first + 4096,
            "the journal grew to {largest} bytes from {first}"
        );
        assert!(core.job_revision() > revision);

        // A crash may tear the last line; it is ignored.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, b"{\"progress\":{\"owner\""))
            .unwrap();
        let (records, clean) = read_journals(&scratch.0.join("blobs")).unwrap();
        assert!(clean);
        assert_eq!(records.len(), 1);
        let snapshot = records[0].snapshot();
        assert_eq!(snapshot.files_done, FILES as u64);
        assert_eq!(snapshot.done_bytes, FILES as u64);
        assert_eq!(
            records[0].unsaved_landed().len(),
            0,
            "landed files read back as saved"
        );
        let landed = records[0].test_landed();
        assert_eq!(landed.len(), FILES);
        assert!((0..FILES).all(|index| landed[&index] == format!("file-{index:05}.bin")));

        // A forgotten job's journal goes with it.
        lock(&core.shared.jobs).remove(&id);
        save_jobs(&core.shared).unwrap();
        assert!(!path.exists());
        core.shutdown();
    }
}
