// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The log file: one folder per run, named by its UTC start, `engine.log`
//! inside it, its previous content kept as `engine.log.1` when it grows past
//! [`FILE_BYTES`], and the oldest runs' folders removed past [`RUNS_KEPT`].

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The log file's size before it starts again.
const FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Runs whose folders are kept, this one included.
const RUNS_KEPT: usize = 10;

const FILE_NAME: &str = "engine.log";

pub(crate) struct LogFile {
    path: PathBuf,
    file: File,
    written: u64,
}

impl LogFile {
    /// A new run's folder under `root`, and its log file.
    pub(crate) fn open_run(root: &Path) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        let stamp = run_name(SystemTime::now());
        let mut folder = root.join(&stamp);
        let mut suffix = 1;
        while folder.exists() {
            suffix += 1;
            folder = root.join(format!("{stamp}-{suffix}"));
        }
        fs::create_dir(&folder)?;
        prune(root, RUNS_KEPT)?;
        let path = folder.join(FILE_NAME);
        let file = File::create(&path)?;
        Ok(Self {
            path,
            file,
            written: 0,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn write(&mut self, line: &str) {
        if self.written + line.len() as u64 > FILE_BYTES {
            let mut previous = self.path.clone().into_os_string();
            previous.push(".1");
            if fs::rename(&self.path, previous).is_err() {
                return;
            }
            match File::create(&self.path) {
                Ok(file) => {
                    self.file = file;
                    self.written = 0;
                }
                Err(_) => return,
            }
        }
        if writeln!(self.file, "{line}").is_ok() {
            self.written += line.len() as u64 + 1;
        }
    }
}

/// `YYYYMMDDTHHMMSS` in UTC.
pub(crate) fn run_name(now: SystemTime) -> String {
    let seconds = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (year, month, day) = civil(seconds / 86_400);
    let time = seconds % 86_400;
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}",
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

/// The calendar date of a day counted from 1970-01-01 (Howard Hinnant's
/// `civil_from_days`, for days on or after the epoch).
fn civil(days: u64) -> (u64, u64, u64) {
    let days = days + 719_468;
    let era = days / 146_097;
    let day_of_era = days % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// Removes the oldest run folders under `root` past `kept`. Only folders
/// named as runs are touched.
pub(crate) fn prune(root: &Path, kept: usize) -> io::Result<()> {
    let mut runs: Vec<PathBuf> = fs::read_dir(root)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| entry.file_name().to_str().is_some_and(is_run_name))
        .map(|entry| entry.path())
        .collect();
    if runs.len() <= kept {
        return Ok(());
    }
    runs.sort();
    for run in &runs[..runs.len() - kept] {
        let _ = fs::remove_dir_all(run);
    }
    Ok(())
}

fn is_run_name(name: &str) -> bool {
    let stamp = name.split('-').next().unwrap_or_default();
    stamp.len() == 15
        && stamp.char_indices().all(|(index, character)| {
            if index == 8 {
                character == 'T'
            } else {
                character.is_ascii_digit()
            }
        })
}

/// At most `max_bytes` from the end of `path`, from the first whole line.
pub(crate) fn tail(path: &Path, max_bytes: usize) -> io::Result<String> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(max_bytes as u64);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity(usize::try_from(length - start).unwrap_or(max_bytes));
    file.take(max_bytes as u64).read_to_end(&mut bytes)?;
    if start > 0 {
        let first = bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |at| at + 1);
        bytes.drain(..first);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
