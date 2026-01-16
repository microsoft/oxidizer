// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for `JoinHandle` implementations.

use arty::{JoinHandle, Spawner};

static_assertions::assert_impl_all!(JoinHandle<()>: Send, Sync);

#[cfg(feature = "tokio")]
#[tokio::test]
async fn join_handle_debug() {
    let spawner = Spawner::tokio();
    let handle = spawner.spawn(async { 42 });
    let debug_str = format!("{handle:?}");
    assert!(debug_str.contains("JoinHandle"));
    let _ = handle.await;
}
