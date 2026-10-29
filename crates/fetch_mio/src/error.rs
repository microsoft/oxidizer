// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;
use std::net::SocketAddr;
use std::{fmt, io};

use http_extensions::HttpError;
use ohno::ErrorLabel;
use seatbelt::RecoveryInfo;

pub(crate) const LABEL_CONNECT: ErrorLabel = ErrorLabel::from_static("connect");
pub(crate) const LABEL_DNS: ErrorLabel = ErrorLabel::from_static("dns");
pub(crate) const LABEL_REGISTRATION: ErrorLabel = ErrorLabel::from_static("registration");
pub(crate) const LABEL_RUNTIME: ErrorLabel = ErrorLabel::from_static("runtime");
pub(crate) const LABEL_SCHEME_NOT_ALLOWED: ErrorLabel = ErrorLabel::from_static("scheme_not_allowed");
pub(crate) const LABEL_SOCKET_OPTION: ErrorLabel = ErrorLabel::from_static("socket_option");

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AttemptPhase {
    Connect,
    Registration,
    SocketOption,
}

impl AttemptPhase {
    const fn label(&self) -> ErrorLabel {
        match self {
            Self::Connect => LABEL_CONNECT,
            Self::Registration => LABEL_REGISTRATION,
            Self::SocketOption => LABEL_SOCKET_OPTION,
        }
    }
}

impl fmt::Display for AttemptPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Connect => "connect",
            Self::Registration => "registration",
            Self::SocketOption => "socket option",
        })
    }
}

#[derive(Debug)]
pub(crate) struct AttemptFailure {
    address: SocketAddr,
    phase: AttemptPhase,
    error: io::Error,
}

impl AttemptFailure {
    pub(crate) const fn new(address: SocketAddr, phase: AttemptPhase, error: io::Error) -> Self {
        Self { address, phase, error }
    }

    pub(crate) const fn error(&self) -> &io::Error {
        &self.error
    }

    pub(crate) const fn phase(&self) -> &AttemptPhase {
        &self.phase
    }
}

impl fmt::Display for AttemptFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed for {}: {}", self.phase, self.address, self.error)
    }
}

impl Error for AttemptFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.error)
    }
}

#[derive(Debug)]
struct ConnectFailures {
    failures: Vec<AttemptFailure>,
}

impl fmt::Display for ConnectFailures {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let last = self.failures.last();
        write!(f, "all {} connection attempts failed", self.failures.len())?;
        if let Some(last) = last {
            write!(f, ", last failure: {last}")?;
        }
        Ok(())
    }
}

impl Error for ConnectFailures {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.failures.last().map(|failure| failure as &(dyn Error + 'static))
    }
}

pub(crate) fn io_error(error: io::Error, label: ErrorLabel) -> HttpError {
    let recovery = RecoveryInfo::from(error.kind());
    HttpError::other(error, recovery, label)
}

pub(crate) fn unavailable(error: impl Into<Box<dyn Error + Send + Sync>>, label: ErrorLabel) -> HttpError {
    HttpError::other(error, RecoveryInfo::unavailable(), label)
}

pub(crate) fn connect_failures(failures: Vec<AttemptFailure>) -> HttpError {
    let Some(last) = failures.last() else {
        return HttpError::other(
            "connection attempts ended without a result",
            RecoveryInfo::unavailable(),
            LABEL_RUNTIME,
        );
    };
    let recovery = RecoveryInfo::from(last.error().kind());
    let label = match last.phase() {
        AttemptPhase::Connect => LABEL_CONNECT,
        phase => ErrorLabel::from_parts([LABEL_CONNECT, phase.label()]),
    };
    HttpError::other(ConnectFailures { failures }, recovery, label)
}
