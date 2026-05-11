// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "test module")]
#![cfg(not(miri))]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyspawn::{BoxedBlockingTask, BoxedFuture, CustomSpawnerBuilder};

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
async fn builder_layer_counts_invocations() {
    let future_count = Arc::new(AtomicUsize::new(0));
    let blocking_count = Arc::new(AtomicUsize::new(0));
    let fc = Arc::clone(&future_count);
    let bc = Arc::clone(&blocking_count);

    let spawner = CustomSpawnerBuilder::tokio()
        .layer(
            move |task: BoxedFuture| -> BoxedFuture {
                fc.fetch_add(1, Ordering::SeqCst);
                task
            },
            move |task: BoxedBlockingTask| -> BoxedBlockingTask {
                bc.fetch_add(1, Ordering::SeqCst);
                task
            },
        )
        .build();

    let r1 = spawner.spawn(async { 1 }).await;
    let r2 = spawner.spawn(async { 2 }).await;
    let r3 = spawner.spawn_blocking(|| 3).await;

    assert_eq!((r1, r2, r3), (1, 2, 3));
    // Each layer runs only for its own task kind.
    assert_eq!(future_count.load(Ordering::SeqCst), 2);
    assert_eq!(blocking_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn builder_stacked_layers() {
    let future_order = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));
    let blocking_order = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));
    let inner_future = Arc::clone(&future_order);
    let outer_future = Arc::clone(&future_order);
    let inner_blocking = Arc::clone(&blocking_order);
    let outer_blocking = Arc::clone(&blocking_order);

    let spawner = CustomSpawnerBuilder::tokio()
        .layer(
            move |task: BoxedFuture| -> BoxedFuture {
                let inner = Arc::clone(&inner_future);
                Box::pin(async move {
                    inner.lock().unwrap().push("inner");
                    task.await;
                })
            },
            move |task: BoxedBlockingTask| -> BoxedBlockingTask {
                let inner = Arc::clone(&inner_blocking);
                Box::new(move || {
                    inner.lock().unwrap().push("inner");
                    task();
                })
            },
        )
        .layer(
            move |task: BoxedFuture| -> BoxedFuture {
                let outer = Arc::clone(&outer_future);
                Box::pin(async move {
                    outer.lock().unwrap().push("outer");
                    task.await;
                })
            },
            move |task: BoxedBlockingTask| -> BoxedBlockingTask {
                let outer = Arc::clone(&outer_blocking);
                Box::new(move || {
                    outer.lock().unwrap().push("outer");
                    task();
                })
            },
        )
        .build();

    spawner.spawn(async {}).await;
    spawner.spawn_blocking(|| {}).await;

    // Layers wrap the task outside-in as added: the first-added (innermost
    // wrapper) layer's pre-task code runs first when the task executes.
    assert_eq!(*future_order.lock().unwrap(), vec!["inner", "outer"]);
    assert_eq!(*blocking_order.lock().unwrap(), vec!["inner", "outer"]);
}

#[tokio::test]
async fn builder_passthrough_layer() {
    let spawner = CustomSpawnerBuilder::tokio()
        .layer(
            |task: BoxedFuture| -> BoxedFuture { task },
            |task: BoxedBlockingTask| -> BoxedBlockingTask { task },
        )
        .build();

    let result = spawner.spawn(async { "hello" }).await;
    assert_eq!(result, "hello");
}

#[tokio::test]
async fn builder_custom_name() {
    let spawner = CustomSpawnerBuilder::tokio().name("my-runtime").build();

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

#[tokio::test]
async fn builder_spawn_anywhere_applies_layer() {
    let count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&count);

    let spawner = CustomSpawnerBuilder::tokio()
        .layer(
            move |task: BoxedFuture| -> BoxedFuture {
                count_clone.fetch_add(1, Ordering::SeqCst);
                task
            },
            |task: BoxedBlockingTask| -> BoxedBlockingTask { task },
        )
        .build();

    // spawn_anywhere exercises TokioSpawner::spawn_anywhere, Layered::spawn_anywhere,
    // and LayeredTask through the builder pipeline.
    let result = spawner.spawn_anywhere(42_i32, |x| async move { x + 1 }).await;
    assert_eq!(result, 43);
    assert_eq!(count.load(Ordering::SeqCst), 1, "layer must be applied to spawn_anywhere tasks");
}

#[tokio::test]
async fn builder_relocate_preserves_layer() {
    use thread_aware::ThreadAware;
    use thread_aware::affinity::pinned_affinities;

    let count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&count);

    let mut spawner = CustomSpawnerBuilder::tokio()
        .layer(
            move |task: BoxedFuture| -> BoxedFuture {
                count_clone.fetch_add(1, Ordering::SeqCst);
                task
            },
            |task: BoxedBlockingTask| -> BoxedBlockingTask { task },
        )
        .build();

    let affinities = pinned_affinities(&[2]);
    spawner.relocate(Some(affinities[0]), affinities[1]);

    // After relocation, the layer must still be applied.
    let result = spawner.spawn(async { 99 }).await;
    assert_eq!(result, 99);
    assert_eq!(count.load(Ordering::SeqCst), 1, "layer must still work after relocate");
}

#[tokio::test]
async fn builder_custom_spawner_new() {
    // Exercises CustomSpawnerBuilder::new (non-tokio constructor)
    use anyspawn::SpawnCustom;
    use thread_aware::ThreadAware;
    use thread_aware::affinity::Affinity;

    #[derive(Clone)]
    struct InlineSpawner;

    impl ThreadAware for InlineSpawner {
        fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {}
    }

    impl SpawnCustom for InlineSpawner {
        fn spawn(&self, task: BoxedFuture) {
            futures::executor::block_on(task);
        }

        fn spawn_anywhere(&self, task: Box<dyn thread_aware::closure::ThreadAwareAsyncFnOnce<()>>) {
            futures::executor::block_on(task.call_once());
        }

        fn spawn_blocking(&self, task: anyspawn::BoxedBlockingTask) {
            task();
        }
    }

    let spawner = CustomSpawnerBuilder::new(InlineSpawner).name("inline").build();
    let dbg = format!("{spawner:?}");
    assert!(dbg.contains("inline"), "Debug should contain the custom name: {dbg}");
}

#[tokio::test]
async fn spawner_tokio_spawn_anywhere() {
    // Exercises Spawner::spawn_anywhere via plain tokio (non-layered)
    let spawner = CustomSpawnerBuilder::tokio().build();
    let result = spawner.spawn_anywhere(42_i32, |x| async move { x + 1 }).await;
    assert_eq!(result, 43);
}

#[tokio::test]
async fn spawner_tokio_debug_no_handle() {
    // Exercises Debug for SpawnerKind::Tokio(None) branch
    let spawner = CustomSpawnerBuilder::tokio().build();
    let dbg = format!("{spawner:?}");
    assert!(dbg.contains("tokio"), "Debug should mention tokio: {dbg}");
}

#[tokio::test]
async fn spawner_tokio_with_handle_spawn_anywhere() {
    // Exercises spawn_anywhere via Tokio(Some(handle)) branch
    let handle = tokio::runtime::Handle::current();
    let spawner = CustomSpawnerBuilder::tokio_with_handle(handle).build();
    let result = spawner.spawn_anywhere(10_i32, |x| async move { x * 2 }).await;
    assert_eq!(result, 20);
}
