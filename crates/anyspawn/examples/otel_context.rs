// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Using `CustomSpawnerBuilder` to propagate OpenTelemetry context across
//! spawned tasks.
//!
//! When you spawn a task with Tokio, the new task starts with an empty
//! OpenTelemetry [`Context`]. This example adds a layer that captures the
//! caller's current context and reattaches it inside the spawned future,
//! so spans, baggage, and other context values propagate automatically.

use anyspawn::{BoxedFuture, CustomSpawnerBuilder};
use opentelemetry::Context;
use opentelemetry::context::FutureExt as OtelFutureExt;

#[tokio::main]
async fn main() {
    let spawner = CustomSpawnerBuilder::tokio()
        .layer("otel-context", |fut: BoxedFuture, spawn: &dyn Fn(BoxedFuture)| {
            let cx = Context::current();
            spawn(Box::pin(fut.with_context(cx)));
        })
        .build();

    // --- demonstrate that context flows through ---

    // Set a value on the current context.
    let parent_cx = Context::current().with_value(RequestId("abc-123".into()));
    let _guard = parent_cx.attach();

    let result = spawner
        .spawn(async {
            // Inside the spawned task the context is available thanks to
            // the layer above.
            let cx = Context::current();
            let id = cx.get::<RequestId>().map(|r| r.0.as_str()).unwrap_or("<missing>");
            println!("spawned task sees RequestId = {id}");
            id == "abc-123"
        })
        .await;

    assert!(result, "context should have propagated into the spawned task");
    println!("context propagation works!");
}

/// A tiny context value used for demonstration purposes.
#[derive(Debug, Clone)]
struct RequestId(String);
