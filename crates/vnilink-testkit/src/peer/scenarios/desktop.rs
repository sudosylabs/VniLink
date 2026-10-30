// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! `desktop`: the agent serves a scripted screen through its real picture
//! pipeline with the stand-in encoder (`crate::desktop`), shared to be seen
//! only, and says on its clock when the screen started, so the suite can
//! time each change's arrival on its own.

use std::time::Duration;

use serde_json::{json, Value};
use vnilink::Grants;

use super::{released, whole, Scenario, NOTHING};
use crate::desktop::{install_screen, ScreenPipeline, Script, ScriptedScreen, DISPLAY};
use crate::peer::clock;
use crate::Engine;

/// How long the last scenario's viewer may take to go.
const RELEASE_WAIT: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(super) struct DesktopHost {
    pipeline: Option<ScreenPipeline>,
}

/// The screen `params` ask for: `{"script": "still"}`, or
/// `{"script": "ticking", "every_ms": 1..=10000, "share_percent": 1..=100}`.
fn script(params: &Value) -> Result<Script, String> {
    match params.get("script").and_then(Value::as_str) {
        Some("still") => Ok(Script::Still),
        Some("ticking") => Ok(Script::Ticking {
            every: Duration::from_millis(whole(params, "every_ms", 1..=10_000, 100)?),
            share: whole(params, "share_percent", 1..=100, 30)? as f32 / 100.0,
            regions: None,
        }),
        _ => Err("the desktop scenario's script is still or ticking".to_owned()),
    }
}

impl Scenario for DesktopHost {
    fn start(&mut self, host: &Engine, params: &Value) -> Result<Value, String> {
        let script = script(params)?;
        released(host, RELEASE_WAIT)?;
        install_screen(&host.core).map_err(|error| error.to_string())?;
        host.core
            .set_desktop_sharing(Grants {
                view: true,
                ..NOTHING
            })
            .map_err(|error| error.to_string())?;
        let pipeline = ScreenPipeline::start(&host.core, ScriptedScreen::new(script));
        let started_us = clock::us_at(pipeline.screen_started);
        self.pipeline = Some(pipeline);
        Ok(json!({ "started_us": started_us, "display": DISPLAY }))
    }

    fn wait(&mut self, _host: &Engine, what: &str, _within: Duration) -> Result<Value, String> {
        Err(format!("the desktop scenario cannot wait for {what:?}"))
    }

    fn stop(&mut self, host: &Engine) {
        if let Err(error) = host.core.set_desktop_sharing(NOTHING) {
            eprintln!("the agent could not stop sharing: {error}");
        }
        if let Some(Err(error)) = self.pipeline.take().map(ScreenPipeline::finish) {
            eprintln!("{error}");
        }
    }
}
