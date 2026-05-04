// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use anyspawn::{BoxedFuture, CustomSpawnerBuilder};

#[tokio::test]
async fn builder_tokio_basic() {
    let spawner = CustomSpawnerBuilder::tokio().build();
    let result = spawner.spawn(async { 42 }).await;
    assert_eq!(result, 42);
}

#[tokio::test]
async fn builder_tokio_with_handle() {
    let handle = tokio::runtime::Handle::current();
    let spawner = CustomSpawnerBuilder::tokio_with_handle(handle).build();
    let result = spawner.spawn(async { 99 }).await;
    assert_eq!(result, 99);
}

#[tokio::test]
async fn builder_with_counting_layer() {
    let count = Arc::new(AtomicUsize::new(0));
    let count_clone = count.clone();

    let spawner = CustomSpawnerBuilder::tokio()
        .layer(move |task: BoxedFuture| -> BoxedFuture {
            count_clone.fetch_add(1, Ordering::SeqCst);
            task
        })
        .build();

    let r1 = spawner.spawn(async { 1 }).await;
    let r2 = spawner.spawn(async { 2 }).await;
    let r3 = spawner.spawn(async { 3 }).await;

    assert_eq!(r1, 1);
    assert_eq!(r2, 2);
    assert_eq!(r3, 3);
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn builder_stacked_layers() {
    let outer_count = Arc::new(AtomicUsize::new(0));
    let inner_count = Arc::new(AtomicUsize::new(0));

    let outer_clone = outer_count.clone();
    let inner_clone = inner_count.clone();

    let spawner = CustomSpawnerBuilder::tokio()
        .layer(move |task: BoxedFuture| -> BoxedFuture {
            inner_clone.fetch_add(1, Ordering::SeqCst);
            task
        })
        .layer(move |task: BoxedFuture| -> BoxedFuture {
            outer_clone.fetch_add(1, Ordering::SeqCst);
            task
        })
        .build();

    spawner.spawn(async {}).await;

    assert_eq!(outer_count.load(Ordering::SeqCst), 1);
    assert_eq!(inner_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn builder_passthrough_layer() {
    let spawner = CustomSpawnerBuilder::tokio()
        .layer(|task: BoxedFuture| -> BoxedFuture { task })
        .build();

    let result = spawner.spawn(async { "hello" }).await;
    assert_eq!(result, "hello");
}

#[tokio::test]
async fn builder_custom_name() {
    let spawner = CustomSpawnerBuilder::tokio()
        .name("my-runtime")
        .build();

    let debug = format!("{spawner:?}");
    assert!(debug.contains("my-runtime"));
}

#[tokio::test]
async fn builder_debug() {
    let builder = CustomSpawnerBuilder::tokio();
    let debug = format!("{builder:?}");
    assert!(debug.contains("CustomSpawnerBuilder"));
    assert!(debug.contains("tokio"));
}
