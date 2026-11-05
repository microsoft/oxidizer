// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "test code")]
#![cfg(not(miri))] // miri doesn't work well with `insta` snapshots

//! Tests for `Spawner` implementations.

use anyspawn::{Spawner, TokioDeps};
use thread_aware::ThreadAware;

static_assertions::assert_impl_all!(Spawner: Send, Sync, ThreadAware);

#[tokio::test]
async fn tokio_spawn_and_await() {
    let spawner = Spawner::new_tokio(TokioDeps::default());
    let result = spawner.spawn(async { 42 }).await.unwrap();
    assert_eq!(result, 42);
}

#[tokio::test]
async fn tokio_spawn_fire_and_forget() {
    let spawner = Spawner::new_tokio(TokioDeps::new());
    let (tx, rx) = tokio::sync::oneshot::channel();
    let () = spawner
        .spawn(async move {
            tx.send(42).unwrap();
        })
        .await
        .unwrap();

    assert_eq!(rx.await.unwrap(), 42);
}

#[test]
fn tokio_with_handle_spawn_and_await() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let spawner = Spawner::new_tokio(rt.handle());

    // Spawning with an explicit handle works even outside a Tokio runtime context.
    let result = rt.block_on(spawner.spawn(async { 42 })).unwrap();
    assert_eq!(result, 42);
}

#[test]
fn tokio_with_handle_spawner_debug() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let spawner = Spawner::new_tokio(rt.handle().clone());
    let debug_str = format!("{spawner:?}");
    assert_eq!(debug_str, r#"Spawner("tokio(handle)")"#);
}

#[tokio::test]
async fn tokio_spawn_anywhere() {
    let spawner = Spawner::new_tokio(TokioDeps::new());
    let result = spawner.spawn_anywhere(42_i32, |x| async move { x + 1 }).await.unwrap();
    assert_eq!(result, 43);
}

#[test]
fn tokio_with_handle_spawn_anywhere() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let spawner = Spawner::new_tokio(TokioDeps::with_handle(rt.handle().clone()));
    let result = rt.block_on(spawner.spawn_anywhere(10_i32, |x| async move { x * 2 })).unwrap();
    assert_eq!(result, 20);
}

#[tokio::test]
async fn tokio_spawner_debug() {
    let spawner = Spawner::new_tokio(TokioDeps::new());
    let debug_str = format!("{spawner:?}");
    assert_eq!(debug_str, r#"Spawner("tokio")"#);
}

#[tokio::test]
async fn tokio_spawn_blocking() {
    let spawner = Spawner::new_tokio(TokioDeps::new());
    let result = spawner.spawn_blocking(|| 21 * 2).await.unwrap();
    assert_eq!(result, 42);
}

#[test]
fn tokio_with_handle_spawn_blocking() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let spawner = Spawner::new_tokio(TokioDeps::with_handle(rt.handle().clone()));
    let result = rt.block_on(spawner.spawn_blocking(|| 7 * 6)).unwrap();
    assert_eq!(result, 42);
}

#[test]
fn tokio_handle_spawner_preserves_its_runtime_after_relocation() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let original = Spawner::new_tokio(TokioDeps::with_handle(runtime.handle().clone()));
    let mut relocated = original.clone();
    _ = thread_aware::Relocator::between_threads().relocate(&mut relocated);

    assert_eq!(futures::executor::block_on(original.spawn(async { 41 })).unwrap(), 41);
    assert_eq!(futures::executor::block_on(relocated.spawn(async { 42 })).unwrap(), 42);
}
