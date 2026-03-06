// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "test code")]
#![cfg(all(feature = "tokio", feature = "custom"))]

//! Tests for `CustomSpawnerBuilder` naming and debug output.

use anyspawn::{BoxedFuture, CustomSpawnerBuilder};

#[test]
fn builder_debug_shows_name() {
    let builder = CustomSpawnerBuilder::tokio();
    let debug = format!("{builder:?}");
    assert!(debug.contains("tokio"), "expected 'tokio' in: {debug}");
}

#[test]
fn builder_debug_shows_layer_names() {
    let builder = CustomSpawnerBuilder::tokio()
        .layer("tracing", |fut: BoxedFuture, spawn: &dyn Fn(BoxedFuture)| {
            spawn(fut);
        })
        .layer("metrics", |fut: BoxedFuture, spawn: &dyn Fn(BoxedFuture)| {
            spawn(fut);
        });

    let debug = format!("{builder:?}");
    assert!(debug.contains("tracing"), "expected 'tracing' in: {debug}");
    assert!(debug.contains("metrics"), "expected 'metrics' in: {debug}");
}

#[test]
fn builder_debug_no_layers_field_when_empty() {
    let builder = CustomSpawnerBuilder::tokio();
    let debug = format!("{builder:?}");
    assert!(!debug.contains("layers"), "expected no 'layers' in: {debug}");
}

#[test]
fn spawner_debug_shows_custom_name() {
    let spawner = CustomSpawnerBuilder::custom("my-runtime", |_: BoxedFuture| {}).build();
    let debug = format!("{spawner:?}");
    assert!(
        debug.contains("my-runtime"),
        "expected 'my-runtime' in: {debug}"
    );
}

#[test]
fn spawner_debug_shows_layers() {
    let spawner = CustomSpawnerBuilder::tokio()
        .layer("otel-context", |fut: BoxedFuture, spawn: &dyn Fn(BoxedFuture)| {
            spawn(fut);
        })
        .layer("panic-handler", |fut: BoxedFuture, spawn: &dyn Fn(BoxedFuture)| {
            spawn(fut);
        })
        .build();

    let debug = format!("{spawner:?}");
    assert!(
        debug.contains("otel-context"),
        "expected 'otel-context' in: {debug}"
    );
    assert!(
        debug.contains("panic-handler"),
        "expected 'panic-handler' in: {debug}"
    );
    assert!(debug.contains("tokio"), "expected 'tokio' in: {debug}");
}

#[test]
fn spawner_debug_no_layers_when_empty() {
    let spawner = CustomSpawnerBuilder::tokio().build();
    let debug = format!("{spawner:?}");
    assert!(!debug.contains("layers"), "expected no 'layers' in: {debug}");
}

#[tokio::test]
async fn layered_spawner_still_works() {
    let spawner = CustomSpawnerBuilder::tokio()
        .layer("passthrough", |fut: BoxedFuture, spawn: &dyn Fn(BoxedFuture)| {
            spawn(fut);
        })
        .build();

    let result = spawner.spawn(async { 42 }).await;
    assert_eq!(result, 42);
}
