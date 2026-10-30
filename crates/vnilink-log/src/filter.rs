// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Which events the log takes: one directive string, from the environment,
//! the application's setting or the default, and the recorder's filter that
//! follows from it.

use tracing_subscriber::EnvFilter;

/// VniLink's information and libraries' warnings.
pub const DEFAULT_FILTER: &str = "warn,vnilink=info";

/// The longest filter accepted.
pub const MAX_FILTER_BYTES: usize = 1024;

/// The engine's span (`vnilink::trace::ENGINE`).
const ENGINE: &str = "vnilink::engine";

/// What the flight recorder keeps whatever the filter, unless it is `off`.
const RECORDED: &str = "vnilink=debug";

/// Where the filter in force came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterSource {
    Environment,
    Setting,
    Default,
}

impl FilterSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::Setting => "setting",
            Self::Default => "default",
        }
    }
}

/// The filter in force.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterState {
    pub directives: String,
    pub source: FilterSource,
}

/// A filter that is too long or that `tracing` cannot read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidFilter;

impl std::fmt::Display for InvalidFilter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("The log filter is not valid")
    }
}

impl std::error::Error for InvalidFilter {}

pub(crate) struct Filters {
    environment: Option<String>,
    rejected_environment: bool,
    setting: Option<String>,
}

fn parse(directives: &str) -> Result<EnvFilter, InvalidFilter> {
    if directives.len() > MAX_FILTER_BYTES {
        return Err(InvalidFilter);
    }
    EnvFilter::try_new(directives).map_err(|_| InvalidFilter)
}

fn given(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

impl Filters {
    /// The environment's filter when it is set and valid.
    pub(crate) fn from_environment(value: Option<&str>) -> Self {
        let value = given(value);
        let environment = value.filter(|value| parse(value).is_ok());
        Self {
            environment: environment.map(str::to_owned),
            rejected_environment: value.is_some() && environment.is_none(),
            setting: None,
        }
    }

    pub(crate) fn rejected_environment(&self) -> bool {
        self.rejected_environment
    }

    pub(crate) fn set(&mut self, setting: Option<&str>) -> Result<(), InvalidFilter> {
        let setting = given(setting);
        if let Some(setting) = setting {
            parse(setting)?;
        }
        self.setting = setting.map(str::to_owned);
        Ok(())
    }

    pub(crate) fn state(&self) -> FilterState {
        let (directives, source) = match (&self.environment, &self.setting) {
            (Some(environment), _) => (environment.clone(), FilterSource::Environment),
            (None, Some(setting)) => (setting.clone(), FilterSource::Setting),
            (None, None) => (DEFAULT_FILTER.to_owned(), FilterSource::Default),
        };
        FilterState { directives, source }
    }

    /// The log file's and standard error's filter. Unless it is `off`, it
    /// always lets the engine's span through, so every line says which
    /// engine recorded it whichever areas the filter chooses.
    pub(crate) fn output(&self) -> EnvFilter {
        let directives = self.state().directives;
        if directives.trim() == "off" {
            return EnvFilter::new("off");
        }
        parse(&format!("{directives},{ENGINE}=error"))
            .unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER))
    }

    /// What the flight recorder keeps besides what [`Self::output`] lets
    /// through: VniLink's debug events, or nothing for `off`.
    pub(crate) fn recorded(&self) -> EnvFilter {
        if self.state().directives.trim() == "off" {
            EnvFilter::new("off")
        } else {
            EnvFilter::new(RECORDED)
        }
    }
}
