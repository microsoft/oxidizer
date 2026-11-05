// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Deterministic connection policy contracts.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use fetch_options::{ConnectionKeepAlive, ConnectionLifetime};

#[test]
fn gamma_survivor_lifetime_debug_and_resolution() {
    let fixed = ConnectionLifetime::fixed(Duration::from_secs(17));
    assert_eq!(format!("{fixed:?}"), "ConnectionLifetime(17s)");
    assert_eq!(fixed.resolve(), Some(Duration::from_secs(17)));
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let generated = ConnectionLifetime::per_connection(move || {
        observed.fetch_add(1, Ordering::Relaxed);
        Some(Duration::from_secs(23))
    });
    assert_eq!(format!("{generated:?}"), "ConnectionLifetime(<closure>)");
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert_eq!(generated.resolve(), Some(Duration::from_secs(23)));
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(matches!(ConnectionKeepAlive::disabled(), ConnectionKeepAlive::Disabled));
}
