// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;
use std::fmt;

/// The Mio driver closed before it reported more readiness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegistrationClosed;

impl fmt::Display for RegistrationClosed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the Arty Mio registration is closed")
    }
}

impl Error for RegistrationClosed {}
