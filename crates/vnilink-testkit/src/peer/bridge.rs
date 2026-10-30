// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! `vnilink-testkit bridge`: the suite's side of a run against the agent
//! for a build script, which speaks JSON lines over this process's input
//! and output instead of linking the kit. It pairs with the agent as a
//! suite does, then answers one line per line it reads, in order:
//!
//! - `{"op":"start","scenario":…,"params":{…}}` → `{"ok":true,"facts":{…}}`
//! - `{"op":"put","name":"viewer.json","data":"<base64>"}` → `{"ok":true}`
//! - `{"op":"poll"}` → `{"ok":true,"files":[{"name","data"}],"running":…}`
//! - `{"op":"fetch","into":"<folder here>"}` → `{"ok":true,"files":[…]}`,
//!   having written every file of the scenario's output under that folder
//! - `{"op":"stop"}` → `{"ok":true,"records":[…]}`
//!
//! A request that fails answers `{"ok":false,"error":…}`. Before any
//! answer it prints `{"event":"approve","digits":…}`, the digits the
//! agent's owner must type, then `{"event":"ready",…}` once paired, or
//! `{"event":"failed","error":…}`. It ends when its input closes.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};

use super::{decode, encode, PeerError, Polled, RemoteHost};
use crate::Engine;

/// One line a build script sends.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Command {
    Start {
        scenario: String,
        #[serde(default)]
        params: Value,
    },
    Put {
        name: String,
        data: String,
    },
    Poll,
    Fetch {
        into: PathBuf,
    },
    Stop,
}

/// Pairs `engine` with the agent that wrote `file` and shows `check`, then
/// answers the commands on `input` on `output` until `input` ends.
pub fn run(
    engine: &Engine,
    file: &[u8],
    check: &str,
    input: impl BufRead,
    mut output: impl Write,
) -> Result<(), String> {
    let mut say = |line: Value| -> Result<(), String> {
        writeln!(output, "{line}")
            .and_then(|()| output.flush())
            .map_err(|error| format!("the build script stopped listening: {error}"))
    };
    let mut announced = Ok(());
    let paired = RemoteHost::pair_announcing(engine, file, check, |digits| {
        announced = say(json!({ "event": "approve", "digits": digits }));
    });
    announced?;
    let mut host = match paired {
        Ok(host) => host,
        Err(error) => {
            say(json!({ "event": "failed", "error": error.to_string() }))?;
            return Err(format!("the agent could not be paired with: {error}"));
        }
    };
    let path = host.path(engine);
    say(json!({
        "event": "ready",
        "platform": host.platform(),
        "path": match &path { Some(path) if path.relay => "relay", Some(_) => "direct", None => "none" },
        "rtt_ms": path.map(|path| path.rtt_ms),
    }))?;
    for line in input.lines() {
        let line = line.map_err(|error| format!("the build script's input failed: {error}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Command>(&line) {
            Ok(command) => answer(&mut host, command),
            Err(error) => Err(format!("not a bridge command: {error}")),
        };
        say(match answer {
            Ok(mut fields) => {
                fields["ok"] = json!(true);
                fields
            }
            Err(error) => json!({ "ok": false, "error": error }),
        })?;
    }
    Ok(())
}

fn answer(host: &mut RemoteHost, command: Command) -> Result<Value, String> {
    let failed = |error: PeerError| error.to_string();
    match command {
        Command::Start { scenario, params } => {
            let facts = host.start(&scenario, params).map_err(failed)?;
            Ok(json!({ "facts": facts }))
        }
        Command::Put { name, data } => {
            let bytes = decode(&data).ok_or_else(|| format!("{name}'s data is not base64"))?;
            host.put(&name, &bytes).map_err(failed)?;
            Ok(json!({}))
        }
        Command::Poll => {
            let Polled { files, running } = host.poll().map_err(failed)?;
            let files: Vec<Value> = files
                .iter()
                .map(|(name, bytes)| json!({ "name": name, "data": encode(bytes) }))
                .collect();
            Ok(json!({ "files": files, "running": running }))
        }
        Command::Fetch { into } => {
            let mut fetched = Vec::new();
            for listed in host.list().map_err(failed)? {
                let target = beneath(&into, &listed.path).ok_or_else(|| {
                    format!(
                        "the agent listed {:?}, which is no relative path",
                        listed.path
                    )
                })?;
                let bytes = host.fetch(&listed).map_err(failed)?;
                if let Some(folder) = target.parent() {
                    std::fs::create_dir_all(folder).map_err(|error| {
                        format!("{} could not be made: {error}", folder.display())
                    })?;
                }
                std::fs::write(&target, bytes).map_err(|error| {
                    format!("{} could not be written: {error}", target.display())
                })?;
                fetched.push(listed.path);
            }
            Ok(json!({ "files": fetched }))
        }
        Command::Stop => {
            let records = host.stop().map_err(failed)?;
            Ok(json!({ "records": records }))
        }
    }
}

/// `path`, plain names joined by `/`, under `folder`; `None` for any other
/// path, so a file the agent names lands only beneath `folder`.
fn beneath(folder: &Path, path: &str) -> Option<PathBuf> {
    let steps = super::folders::steps(path)?;
    Some(
        steps
            .iter()
            .fold(folder.to_path_buf(), |target, step| target.join(step)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file the agent names lands beneath the folder asked for, or
    /// nowhere.
    #[test]
    fn a_fetched_file_lands_only_beneath_its_folder() {
        let folder = Path::new("evidence");
        assert_eq!(
            beneath(folder, "logs/host.log"),
            Some(folder.join("logs").join("host.log"))
        );
        for path in ["../x", "/etc/passwd", "a//b", "a/./b", "", "C:\\x", "c:x"] {
            assert_eq!(beneath(folder, path), None, "{path:?} lands nowhere");
        }
    }
}
