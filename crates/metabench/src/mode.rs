// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::OsStr;
use std::fmt;
use std::str::FromStr;

use crate::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Mode {
    Criterion,
    Callgrind,
    Perf,
    Allocations,
}

impl Mode {
    pub(crate) const ALL: [Self; 4] = [Self::Criterion, Self::Callgrind, Self::Perf, Self::Allocations];

    pub(crate) fn from_os_str(value: &OsStr) -> Result<Self, Error> {
        value
            .to_str()
            .ok_or_else(|| Error::InvalidMode(value.to_string_lossy().into_owned()))?
            .parse()
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Criterion => "criterion",
            Self::Callgrind => "callgrind",
            Self::Perf => "perf",
            Self::Allocations => "allocations",
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Mode {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "criterion" => Ok(Self::Criterion),
            "callgrind" => Ok(Self::Callgrind),
            "perf" => Ok(Self::Perf),
            "allocations" => Ok(Self::Allocations),
            _ => Err(Error::InvalidMode(value.to_owned())),
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_documented_mode() {
        for mode in Mode::ALL {
            assert!(matches!(
                mode.as_str().parse::<Mode>(),
                Ok(parsed) if parsed == mode
            ));
        }
    }

    #[test]
    fn rejects_unknown_mode() {
        assert!(matches!(
            "unknown".parse::<Mode>(),
            Err(Error::InvalidMode(mode)) if mode == "unknown"
        ));
    }
}
