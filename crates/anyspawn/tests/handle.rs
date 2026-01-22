// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "test code")]
#![cfg(any(feature = "tokio", feature = "custom"))]

//! Tests for `JoinHandle` implementations.

use anyspawn::Spawner;

#[cfg(feature = "tokio")]
#[tokio::test]
async fn join_handle_debug() {
    let spawner = Spawner::tokio();
    let handle = spawner.spawn(async { 42 });
    let debug_str = format!("{handle:?}");
    assert!(debug_str.contains("JoinHandle"));
    let _ = handle.await;
}
