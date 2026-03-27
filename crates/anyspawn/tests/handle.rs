// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "test code")]

//! Tests for `JoinHandle` implementations.

use anyspawn::Spawner;

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn join_handle_debug() {
    let spawner = Spawner::new_tokio();
    let handle = spawner.spawn(async { 42 });
    let debug_str = format!("{handle:?}");
    assert!(debug_str.contains("JoinHandle"));
    let _ = handle.await;
}
