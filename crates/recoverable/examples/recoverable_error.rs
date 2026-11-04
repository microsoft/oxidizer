// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Use `Recovery` with a custom error type.
//!
//! `RecoveryInfo` assigns a recovery classification to each error.

use std::error::Error;
use std::fmt::Display;
use std::time::Duration;

use recoverable::{Recovery, RecoveryInfo, RecoveryKind};

fn main() {
    handle_network_error(&NetworkError::DnsResolutionFailed);
    handle_network_error(&NetworkError::InvalidUrl);
    handle_network_error(&NetworkError::ServiceUnavailable { retry_after: None });
}

/// Defines a network error with different recovery classifications.
#[derive(Debug)]
enum NetworkError {
    /// DNS resolution failed. The condition can be transient.
    DnsResolutionFailed,
    /// The URL format is invalid. This error is permanent.
    InvalidUrl,
    /// The service is unavailable. For example, a circuit breaker is open.
    ServiceUnavailable { retry_after: Option<Duration> },
}

impl Recovery for NetworkError {
    fn recovery(&self) -> RecoveryInfo {
        match self {
            Self::DnsResolutionFailed => RecoveryInfo::retry(),
            Self::InvalidUrl => RecoveryInfo::never(),
            Self::ServiceUnavailable { retry_after: Some(after) } => RecoveryInfo::unavailable().delay(*after),
            Self::ServiceUnavailable { retry_after: None } => RecoveryInfo::unavailable(),
        }
    }
}

impl Display for NetworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DnsResolutionFailed => write!(f, "DNS resolution failed"),
            Self::InvalidUrl => write!(f, "invalid URL format"),
            Self::ServiceUnavailable { retry_after } => {
                if let Some(after) = retry_after {
                    write!(f, "service unavailable, retry after {after:?}")
                } else {
                    write!(f, "service unavailable")
                }
            }
        }
    }
}

impl Error for NetworkError {}

/// Handles a network error based on its recovery information.
fn handle_network_error(error: &NetworkError) {
    let recovery = error.recovery();

    println!("\nError: {error}");
    println!("Recovery strategy: {recovery}");

    match recovery.kind() {
        RecoveryKind::Retry => println!("→ transient network issue, retry recommended"),
        RecoveryKind::Unavailable => println!("→ service appears to be down"),
        RecoveryKind::Never => println!("→ configuration or code change needed"),
        RecoveryKind::Unknown => println!("→ unknown recovery status"),
        _ => println!("→ unhandled recovery kind"),
    }
}
