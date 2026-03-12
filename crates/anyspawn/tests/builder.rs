// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "test code")]
#![cfg(all(feature = "tokio", feature = "custom"))]
#![cfg(not(miri))] // miri doesn't work well with `insta` snapshots

//! Tests for `CustomSpawnerBuilder` naming and debug output.

use anyspawn::{BoxedFuture, CustomSpawnerBuilder, Spawner};

#[test]
fn tokio_spawner_debug() {
    let spawner = Spawner::new_tokio();
    insta::assert_snapshot!(format!("{spawner:?}"), @r#"Spawner("tokio")"#);
}

#[test]
fn custom_spawner_debug() {
    let spawner = Spawner::new_custom("my-runtime", |_| {});
    insta::assert_snapshot!(format!("{spawner:?}"), @r#"Spawner(CustomSpawner { name: "my-runtime" })"#);
}

#[test]
fn builder_debug_no_layers() {
    let builder = CustomSpawnerBuilder::tokio();
    insta::assert_snapshot!(format!("{builder:?}"), @r#"CustomSpawnerBuilder { name: "tokio" }"#);
}

#[test]
fn builder_debug_with_layers() {
    let builder = CustomSpawnerBuilder::tokio()
        .layer("tracing", |fut: BoxedFuture, spawn: &dyn Fn(BoxedFuture)| {
            spawn(fut);
        })
        .layer("metrics", |fut: BoxedFuture, spawn: &dyn Fn(BoxedFuture)| {
            spawn(fut);
        });

    insta::assert_snapshot!(format!("{builder:?}"), @r#"CustomSpawnerBuilder { name: "tokio", layers: ["tracing", "metrics"] }"#);
}

#[test]
fn builder_custom_name_debug() {
    let builder = CustomSpawnerBuilder::custom("smol", |_: BoxedFuture| {});
    insta::assert_snapshot!(format!("{builder:?}"), @r#"CustomSpawnerBuilder { name: "smol" }"#);
}

#[test]
fn built_spawner_debug_with_layers() {
    let spawner = CustomSpawnerBuilder::tokio()
        .layer("otel-context", |fut: BoxedFuture, spawn: &dyn Fn(BoxedFuture)| {
            spawn(fut);
        })
        .layer("panic-handler", |fut: BoxedFuture, spawn: &dyn Fn(BoxedFuture)| {
            spawn(fut);
        })
        .build();

    insta::assert_snapshot!(format!("{spawner:?}"), @r#"Spawner(CustomSpawner { name: "tokio", layers: ["otel-context", "panic-handler"] })"#);
}

#[test]
fn built_spawner_debug_no_layers() {
    let spawner = CustomSpawnerBuilder::tokio().build();
    insta::assert_snapshot!(format!("{spawner:?}"), @r#"Spawner(CustomSpawner { name: "tokio" })"#);
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
