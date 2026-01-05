// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Example demonstrating how to use implement `Recovery` trait
//! for errors implemented using the `ohno` crate.

use recoverable::{Recovery, RecoveryInfo, RecoveryKind};

fn main() {
    handle_network_error(&NetworkError::dns_resolution_failed());
    handle_network_error(&NetworkError::invalid_url());
    handle_network_error(&NetworkError::service_unavailable());
}

/// A transparent network error type demonstrating different recovery scenarios.
#[ohno::error]
struct NetworkError {
    recovery_info: RecoveryInfo,
}

impl NetworkError {
    fn dns_resolution_failed() -> Self {
        Self::caused_by(RecoveryInfo::retry(), "DNS resolution failed")
    }

    fn invalid_url() -> Self {
        Self::caused_by(RecoveryInfo::never(), "invalid URL format")
    }

    fn service_unavailable() -> Self {
        Self::caused_by(RecoveryInfo::unavailable(), "service unavailable")
    }
}

impl Recovery for NetworkError {
    fn recovery(&self) -> RecoveryInfo {
        self.recovery_info.clone()
    }
}

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
