// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::OsStr;
use std::fmt;
use std::str::FromStr;

use crate::error::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Mode {
    Criterion,
    Gungraun,
    Perf,
    Vtune,
    Allocations,
}

impl Mode {
    pub(crate) const ALL: [Self; 5] = [Self::Criterion, Self::Gungraun, Self::Perf, Self::Vtune, Self::Allocations];

    /// The engine set used when no engine is explicitly selected.
    ///
    /// Gungraun measures via Valgrind/Callgrind and is only available on
    /// Linux (see [`Self::availability`]), so it is omitted from the implicit
    /// default off Linux to keep the documented no-selector invocation
    /// succeeding on every platform. An explicit `--gungraun` request is
    /// still honored and surfaces [`crate::error::Error::UnsupportedMode`]
    /// when it actually runs.
    pub(crate) fn default_set() -> Vec<Self> {
        if cfg!(target_os = "linux") {
            vec![Self::Criterion, Self::Gungraun, Self::Allocations]
        } else {
            vec![Self::Criterion, Self::Allocations]
        }
    }

    pub(crate) fn from_os_str(value: &OsStr) -> Result<Self, Error> {
        value
            .to_str()
            .ok_or_else(|| Error::InvalidMode(value.to_string_lossy().into_owned()))?
            .parse()
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Criterion => "criterion",
            Self::Gungraun => "gungraun",
            Self::Perf => "perf",
            Self::Vtune => "vtune",
            Self::Allocations => "allocations",
        }
    }
}

impl fmt::Display for Mode {
    #[expect(clippy::renamed_function_params, reason = "the descriptive name improves readability")]
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Mode {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "criterion" => Ok(Self::Criterion),
            "gungraun" => Ok(Self::Gungraun),
            "perf" => Ok(Self::Perf),
            "vtune" => Ok(Self::Vtune),
            "allocations" => Ok(Self::Allocations),
            _ => Err(Error::InvalidMode(value.to_owned())),
        }
    }
}
