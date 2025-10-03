// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Example demonstrating how to use the Recoverable trait with error types.
//!
//! This example shows how to implement the Recoverable trait for custom error types
//! and use `RecoveryInfo` to classify errors as transient or permanent.

use std::error::Error;
use std::fmt::Display;
use std::time::Duration;

use recoverable::{Recoverable, RecoveryInfo, RecoveryKind};

fn main() {
    handle_network_error(&NetworkError::DnsResolutionFailed);
    handle_network_error(&NetworkError::InvalidUrl);
    handle_network_error(&NetworkError::ServiceUnavailable { retry_after: None });
}

/// A network error type demonstrating different recovery scenarios.
#[derive(Debug)]
enum NetworkError {
    /// DNS resolution failed - might be transient
    DnsResolutionFailed,
    /// Invalid URL format - permanent error
    InvalidUrl,
    /// Service is unavailable, for example circuit breaker is open
    ServiceUnavailable { retry_after: Option<Duration> },
}

impl Recoverable for NetworkError {
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

/// Demonstrates handling network errors.
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
