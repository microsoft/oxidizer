// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for `Spawner` implementations.

use arty::Spawner;

#[cfg(feature = "tokio")]
#[tokio::test]
async fn tokio_spawn_and_await() {
    let spawner = Spawner::tokio();
    let result = spawner.spawn(async { 42 }).await;
    assert_eq!(result, 42);
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn tokio_spawn_fire_and_forget() {
    let spawner = Spawner::tokio();
    let (tx, rx) = tokio::sync::oneshot::channel();

    let _ = spawner.spawn(async move {
        tx.send(42).unwrap();
    });

    assert_eq!(rx.await.unwrap(), 42);
}

#[cfg(feature = "custom")]
#[test]
fn custom_spawn_and_await() {
    let spawner = Spawner::custom(|fut| {
        std::thread::spawn(move || futures::executor::block_on(fut));
    });

    let result = futures::executor::block_on(spawner.spawn(async { 42 }));
    assert_eq!(result, 42);
}

#[cfg(feature = "custom")]
#[test]
fn custom_spawn_fire_and_forget() {
    let spawner = Spawner::custom(|fut| {
        std::thread::spawn(move || futures::executor::block_on(fut));
    });

    let (tx, rx) = std::sync::mpsc::channel();

    let _ = spawner.spawn(async move {
        tx.send(42).unwrap();
    });

    assert_eq!(rx.recv().unwrap(), 42);
}

#[cfg(feature = "custom")]
#[test]
fn custom_spawner_debug() {
    let spawner = Spawner::custom(|_| {});
    let debug_str = format!("{spawner:?}");
    assert!(debug_str.contains("CustomSpawner"));
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn join_handle_debug() {
    let spawner = Spawner::tokio();
    let handle = spawner.spawn(async { 42 });
    let debug_str = format!("{handle:?}");
    assert!(debug_str.contains("JoinHandle"));
    let _ = handle.await;
}
