// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "test code")]
#![cfg(any(feature = "tokio", feature = "custom"))]

//! Tests for `Spawner` implementations.

use anyspawn::Spawner;

static_assertions::assert_impl_all!(Spawner: Send, Sync);

#[cfg(feature = "tokio")]
#[tokio::test]
async fn tokio_spawn_and_await() {
    let spawner = Spawner::new_tokio();
    let result = spawner.spawn(async { 42 }).await;
    assert_eq!(result, 42);
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn tokio_spawn_fire_and_forget() {
    let spawner = Spawner::new_tokio();
    let (tx, rx) = tokio::sync::oneshot::channel();

    let () = spawner
        .spawn(async move {
            tx.send(42).unwrap();
        })
        .await;

    assert_eq!(rx.await.unwrap(), 42);
}

#[cfg(feature = "custom")]
#[test]
fn custom_spawn_and_await() {
    let spawner = Spawner::new_custom(|fut| {
        std::thread::spawn(move || futures::executor::block_on(fut));
    });

    let result = futures::executor::block_on(spawner.spawn(async { 42 }));
    assert_eq!(result, 42);
}

#[cfg(feature = "custom")]
#[tokio::test]
async fn custom_spawn_fire_and_forget() {
    let spawner = Spawner::new_custom(|fut| {
        std::thread::spawn(move || futures::executor::block_on(fut));
    });

    let (tx, rx) = std::sync::mpsc::channel();

    let () = spawner
        .spawn(async move {
            tx.send(42).unwrap();
        })
        .await;

    assert_eq!(rx.recv().unwrap(), 42);
}

#[cfg(feature = "custom")]
#[test]
fn custom_spawner_debug() {
    let spawner = Spawner::new_custom(|_| {});
    let debug_str = format!("{spawner:?}");
    assert!(debug_str.contains("CustomSpawner"));
}
