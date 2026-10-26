//! The scenarios compiled into the agent: each the host half of a suite or
//! a measurement. A scenario shares only its own service while it runs and
//! works only in the agent's own folders; the suite names it and passes
//! parameters, never a command or a path.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use vnilink::Grants;

use crate::Engine;

/// Every scenario's name, for the agent's refusal of another.
pub(crate) const NAMES: [&str; 1] = ["files"];

/// The host half of one scenario, run by the agent between Start and Stop.
pub(crate) trait Scenario: Send {
    /// Prepares the agent's engine; the facts the controller half needs.
    fn start(&mut self, host: &Engine, params: &Value) -> Result<Value, String>;
    /// Waits up to `within` for `what`; facts about how it ended.
    fn wait(&mut self, host: &Engine, what: &str, within: Duration) -> Result<Value, String>;
    /// Shares nothing again and removes what the scenario made.
    fn stop(&mut self, host: &Engine);
}

/// The scenario named `name`.
pub(crate) fn named(name: &str) -> Option<Box<dyn Scenario>> {
    match name {
        "files" => Some(Box::new(FilesHost::default())),
        _ => None,
    }
}

/// What a host shares when it shares nothing.
const NOTHING: Grants = Grants {
    view: false,
    control: false,
    clipboard: false,
    transfer: false,
};

/// Receives the files the suite uploads into a fresh folder of the agent's
/// root, over a files session, which the transfer grant alone admits.
#[derive(Default)]
struct FilesHost {
    /// The folder the uploads land in, under the agent's root.
    inbox: Option<String>,
    /// Jobs the agent held when the scenario started.
    earlier: HashSet<u64>,
}

impl Scenario for FilesHost {
    fn start(&mut self, host: &Engine, _params: &Value) -> Result<Value, String> {
        let inbox = format!("inbox-{}", vnilink_log::now_ms());
        std::fs::create_dir_all(host.root().join(&inbox))
            .map_err(|error| format!("the inbox could not be made: {}", error.kind()))?;
        self.inbox = Some(inbox.clone());
        self.earlier = host
            .core
            .job_list()
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|job| job.id)
            .collect();
        host.core
            .set_desktop_sharing(Grants {
                transfer: true,
                ..NOTHING
            })
            .map_err(|error| error.to_string())?;
        Ok(json!({ "root": host.root_id(), "inbox": inbox }))
    }

    fn wait(&mut self, host: &Engine, what: &str, within: Duration) -> Result<Value, String> {
        if what != "received" {
            return Err(format!("the files scenario cannot wait for {what:?}"));
        }
        let started = Instant::now();
        loop {
            let jobs = host.core.job_list().map_err(|error| error.to_string())?;
            let ended = jobs.into_iter().find(|job| {
                !job.sender && !self.earlier.contains(&job.id) && job.state.is_terminal()
            });
            if let Some(job) = ended {
                self.earlier.insert(job.id);
                return Ok(json!({
                    "state": format!("{:?}", job.state),
                    "files": job.files_done,
                    "bytes": job.done_bytes,
                }));
            }
            if started.elapsed() > within {
                return Err(format!("no upload ended within {} s", within.as_secs()));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn stop(&mut self, host: &Engine) {
        if let Err(error) = host.core.set_desktop_sharing(NOTHING) {
            eprintln!("the agent could not stop sharing: {error}");
        }
        if let Some(inbox) = self.inbox.take() {
            let _ = std::fs::remove_dir_all(host.root().join(inbox));
        }
    }
}
