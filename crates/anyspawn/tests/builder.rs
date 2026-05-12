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

    let result = spawner.spawn_blocking(|| 42).await;
    assert_eq!(result, 42);
}

#[tokio::test]
async fn builder_tokio_with_handle() {
    let handle = tokio::runtime::Handle::current();
    let spawner = CustomSpawnerBuilder::tokio_with_handle(handle).build();
    let result = spawner.spawn(async { 99 }).await;
    assert_eq!(result, 99);

    let result = spawner.spawn_blocking(|| 99).await;
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

type SharedOrder = Arc<std::sync::Mutex<Vec<&'static str>>>;

/// Builds a layer pair that records `tag` before delegating to the wrapped
/// task: the future side records to `future_order` and the blocking side to
/// `blocking_order`. Used to verify that layers execute in the documented
/// order.
fn recording_layer(
    future_order: SharedOrder,
    blocking_order: SharedOrder,
    tag: &'static str,
) -> (
    impl Fn(BoxedFuture) -> BoxedFuture + Clone + Send + Sync + 'static,
    impl Fn(BoxedBlockingTask) -> BoxedBlockingTask + Clone + Send + Sync + 'static,
) {
    let future_layer = move |task: BoxedFuture| -> BoxedFuture {
        let order = Arc::clone(&future_order);
        Box::pin(async move {
            order.lock().expect("lock poisoned").push(tag);
            task.await;
        })
    };
    let blocking_layer = move |task: BoxedBlockingTask| -> BoxedBlockingTask {
        let order = Arc::clone(&blocking_order);
        Box::new(move || {
            order.lock().expect("lock poisoned").push(tag);
            task();
        })
    };
    (future_layer, blocking_layer)
}

#[tokio::test]
async fn builder_stacked_layers() {
    // Three layers so the ordering is unambiguous: the documented order
    // must reproduce the exact add-order, not just any consistent order.
    let future_order: SharedOrder = Arc::new(std::sync::Mutex::new(Vec::new()));
    let blocking_order: SharedOrder = Arc::new(std::sync::Mutex::new(Vec::new()));

    let (first_f, first_b) = recording_layer(Arc::clone(&future_order), Arc::clone(&blocking_order), "first");
    let (second_f, second_b) = recording_layer(Arc::clone(&future_order), Arc::clone(&blocking_order), "second");
    let (third_f, third_b) = recording_layer(Arc::clone(&future_order), Arc::clone(&blocking_order), "third");

    let spawner = CustomSpawnerBuilder::tokio()
        .layer(first_f, first_b)
        .layer(second_f, second_b)
        .layer(third_f, third_b)
        .build();

    let body_future_order = Arc::clone(&future_order);
    let body_blocking_order = Arc::clone(&blocking_order);

    spawner
        .spawn(async move {
            body_future_order.lock().expect("lock poisoned").push("task");
        })
        .await;
    spawner
        .spawn_blocking(move || {
            body_blocking_order.lock().expect("lock poisoned").push("task");
        })
        .await;

    // Documented behavior: layers run in the order they were added, then
    // the task body runs last.
    assert_eq!(*future_order.lock().unwrap(), vec!["first", "second", "third", "task"]);
    assert_eq!(*blocking_order.lock().unwrap(), vec!["first", "second", "third", "task"]);
}

#[tokio::test]
async fn builder_stacked_layers_spawn_anywhere() {
    // spawn_anywhere goes through the future layer, so it must observe the
    // same add-order semantics as spawn().
    let order: SharedOrder = Arc::new(std::sync::Mutex::new(Vec::new()));
    // The blocking sink is irrelevant here because spawn_anywhere only
    // exercises the future layer; pass the same Vec to satisfy the helper.
    let (first_f, first_b) = recording_layer(Arc::clone(&order), Arc::clone(&order), "first");
    let (second_f, second_b) = recording_layer(Arc::clone(&order), Arc::clone(&order), "second");
    let (third_f, third_b) = recording_layer(Arc::clone(&order), Arc::clone(&order), "third");

    let spawner = CustomSpawnerBuilder::tokio()
        .layer(first_f, first_b)
        .layer(second_f, second_b)
        .layer(third_f, third_b)
        .build();

    // spawn_anywhere takes a fn pointer, so the task body cannot capture
    // the order Vec. We only assert the layer order here; the existing
    // `builder_spawn_anywhere_applies_layer` test already covers that
    // the task body runs after the layer.
    let result = spawner.spawn_anywhere(0_i32, |x| async move { x + 1 }).await;

    assert_eq!(result, 1);
    assert_eq!(*order.lock().unwrap(), vec!["first", "second", "third"]);
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
