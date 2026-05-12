// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`CustomSpawnerBuilder`] for composing layered spawners.

use std::fmt::Debug;

use crate::Spawner;
use crate::custom::{BoxedBlockingTask, BoxedFuture, SpawnCustom};
use thread_aware::ThreadAware;
use thread_aware::affinity::Affinity;
use thread_aware::closure::ThreadAwareAsyncFnOnce;

/// Internal composition of two layer closures wrapping an inner [`SpawnCustom`].
///
/// `future_layer` transforms futures forwarded to [`SpawnCustom::spawn`] and
/// [`SpawnCustom::spawn_anywhere`]; `blocking_layer` transforms tasks
/// forwarded to [`SpawnCustom::spawn_blocking`]. The builder supplies an
/// identity closure for whichever layer kind is not being added. During
/// relocation only the inner spawner is notified; closures are expected to
/// be stateless (or capture only `Arc`-based state that does not need
/// relocation).
struct Layered<FL, BL, S> {
    future_layer: FL,
    blocking_layer: BL,
    inner: S,
}

impl<FL: Clone, BL: Clone, S: Clone> Clone for Layered<FL, BL, S> {
    fn clone(&self) -> Self {
        Self {
            future_layer: self.future_layer.clone(),
            blocking_layer: self.blocking_layer.clone(),
            inner: self.inner.clone(),
        }
    }
}

impl<FL: Send, BL: Send, S: ThreadAware> ThreadAware for Layered<FL, BL, S> {
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        self.inner.relocate(source, destination);
    }
}

impl<FL, BL, S> SpawnCustom for Layered<FL, BL, S>
where
    FL: Fn(BoxedFuture) -> BoxedFuture + Clone + Send + Sync + 'static,
    BL: Fn(BoxedBlockingTask) -> BoxedBlockingTask + Clone + Send + Sync + 'static,
    S: SpawnCustom + Clone,
{
    fn spawn(&self, task: BoxedFuture) {
        self.inner.spawn((self.future_layer)(task));
    }

    fn spawn_anywhere(&self, task: Box<dyn ThreadAwareAsyncFnOnce<()>>) {
        // Wrap the original task so the inner spawner can relocate it before
        // call_once(). The layer is applied lazily inside call_once() so that
        // the captured ThreadAware data is relocated first.
        let layered = Box::new(LayeredTask {
            task,
            layer: self.future_layer.clone(),
        });
        self.inner.spawn_anywhere(layered);
    }

    fn spawn_blocking(&self, task: BoxedBlockingTask) {
        self.inner.spawn_blocking((self.blocking_layer)(task));
    }
}

/// Wraps a [`ThreadAwareAsyncFnOnce`] with a layer function, deferring
/// `call_once()` until after relocation so the inner spawner can relocate
/// the task's captured data first.
struct LayeredTask<F> {
    task: Box<dyn ThreadAwareAsyncFnOnce<()>>,
    layer: F,
}

impl<F: Send> ThreadAware for LayeredTask<F> {
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        self.task.relocate(source, destination);
    }
}

impl<F> ThreadAwareAsyncFnOnce<()> for LayeredTask<F>
where
    F: Fn(BoxedFuture) -> BoxedFuture + Send + 'static,
{
    fn call_once(self: Box<Self>) -> thread_aware::closure::BoxFuture<'static, ()> {
        let future = self.task.call_once();
        (self.layer)(future)
    }
}

/// Built-in Tokio spawner for use as a base in [`CustomSpawnerBuilder`].
#[cfg(feature = "tokio")]
#[derive(Clone)]
struct TokioSpawner(Option<::tokio::runtime::Handle>);

#[cfg(feature = "tokio")]
impl ThreadAware for TokioSpawner {
    fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {}
}

#[cfg(feature = "tokio")]
impl SpawnCustom for TokioSpawner {
    fn spawn(&self, task: BoxedFuture) {
        match &self.0 {
            Some(h) => {
                h.spawn(task);
            }
            None => {
                ::tokio::spawn(task);
            }
        }
    }

    fn spawn_anywhere(&self, task: Box<dyn ThreadAwareAsyncFnOnce<()>>) {
        self.spawn(task.call_once());
    }

    fn spawn_blocking(&self, task: BoxedBlockingTask) {
        match &self.0 {
            Some(h) => {
                h.spawn_blocking(task);
            }
            None => {
                ::tokio::task::spawn_blocking(task);
            }
        }
    }
}

/// Builds a [`Spawner`] from a base spawner plus zero or more layers.
///
/// 1. Pick a base with [`tokio()`](Self::tokio),
///    [`tokio_with_handle()`](Self::tokio_with_handle), or
///    [`new()`](Self::new).
/// 2. Wrap it with any number of [`layer()`](Self::layer) calls.
/// 3. Call [`build()`](Self::build).
///
/// A layer is a pair of closures that wrap each spawned task before it
/// reaches the base spawner: one wraps futures (used by
/// [`Spawner::spawn`] and [`Spawner::spawn_anywhere`]) and one wraps
/// blocking tasks (used by [`Spawner::spawn_blocking`]). Pass `|t| t` for
/// either side to leave that task kind unchanged.
///
/// Layers run in the order they are added: when a task executes, the
/// first layer added runs first, then the second, and so on, until the
/// task itself runs.
///
/// # Note
///
/// For a plain Tokio spawner with no layers, prefer [`Spawner::new_tokio`]:
/// it uses native Tokio `JoinHandle`s directly. The builder's
/// [`tokio()`](Self::tokio) path uses a oneshot channel for join handles
/// so that layers can be applied, which is slightly less efficient.
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "tokio")]
/// # #[tokio::main]
/// # async fn main() {
/// use anyspawn::{BoxedBlockingTask, BoxedFuture, CustomSpawnerBuilder};
///
/// let spawner = CustomSpawnerBuilder::tokio()
///     .layer(
///         |task: BoxedFuture| -> BoxedFuture {
///             println!("spawning task");
///             task
///         },
///         |task: BoxedBlockingTask| -> BoxedBlockingTask { task },
///     )
///     .build();
///
/// let result = spawner.spawn(async { 42 }).await;
/// assert_eq!(result, 42);
/// # }
/// # #[cfg(not(feature = "tokio"))]
/// # fn main() {}
/// ```
pub struct CustomSpawnerBuilder<S> {
    spawner: S,
    name: &'static str,
}

impl CustomSpawnerBuilder<()> {
    /// Creates a builder using Tokio as the base spawner.
    ///
    /// The spawner is named `"tokio"` in [`Debug`] output.
    ///
    /// # Panics
    ///
    /// The resulting [`Spawner`] will panic if used outside a Tokio runtime.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anyspawn::CustomSpawnerBuilder;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let spawner = CustomSpawnerBuilder::tokio().build();
    /// let result = spawner.spawn(async { 42 }).await;
    /// assert_eq!(result, 42);
    /// # }
    /// ```
    #[cfg(feature = "tokio")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tokio")))]
    #[must_use]
    pub fn tokio() -> CustomSpawnerBuilder<impl SpawnCustom + Clone> {
        CustomSpawnerBuilder {
            spawner: TokioSpawner(None),
            name: "tokio",
        }
    }

    /// Creates a builder using an explicit Tokio runtime handle as the base
    /// spawner.
    ///
    /// Unlike [`tokio()`](Self::tokio), this does not require an ambient Tokio
    /// runtime context. Tasks are spawned directly on the provided
    /// [`Handle`](::tokio::runtime::Handle).
    ///
    /// The spawner is named `"tokio"` in [`Debug`] output.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anyspawn::CustomSpawnerBuilder;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let handle = tokio::runtime::Handle::current();
    /// let spawner = CustomSpawnerBuilder::tokio_with_handle(handle).build();
    /// let result = spawner.spawn(async { 42 }).await;
    /// assert_eq!(result, 42);
    /// # }
    /// ```
    #[cfg(feature = "tokio")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tokio")))]
    #[must_use]
    pub fn tokio_with_handle(handle: ::tokio::runtime::Handle) -> CustomSpawnerBuilder<impl SpawnCustom + Clone> {
        CustomSpawnerBuilder {
            spawner: TokioSpawner(Some(handle)),
            name: "tokio",
        }
    }

    /// Creates a builder with a custom base spawner.
    ///
    /// The spawner is named `"custom"` by default in [`Debug`] output.
    /// Use [`name()`](CustomSpawnerBuilder::name) to override the name.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use anyspawn::CustomSpawnerBuilder;
    ///
    /// let spawner = CustomSpawnerBuilder::new(MySpawner::new())
    ///     .name("my-runtime")
    ///     .build();
    /// ```
    pub fn new<S: SpawnCustom + Clone>(base: S) -> CustomSpawnerBuilder<S> {
        CustomSpawnerBuilder {
            spawner: base,
            name: "custom",
        }
    }
}

impl<S: SpawnCustom + Clone> CustomSpawnerBuilder<S> {
    /// Sets the name of the spawner shown in [`Debug`] output.
    #[must_use]
    pub fn name(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    /// Wraps each spawned task with a pair of layer closures before it
    /// reaches the base spawner.
    ///
    /// - `future_layer` wraps the future used by [`Spawner::spawn`] and
    ///   [`Spawner::spawn_anywhere`].
    /// - `blocking_layer` wraps the closure used by
    ///   [`Spawner::spawn_blocking`].
    ///
    /// Pass `|t| t` for either side to leave that task kind unchanged.
    ///
    /// When multiple layers are added, they run in the order they were
    /// added: the first layer added runs first when the task executes.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "tokio")]
    /// # #[tokio::main]
    /// # async fn main() {
    /// use anyspawn::{BoxedBlockingTask, BoxedFuture, CustomSpawnerBuilder};
    ///
    /// let spawner = CustomSpawnerBuilder::tokio()
    ///     .layer(
    ///         |task: BoxedFuture| -> BoxedFuture { task },
    ///         |task: BoxedBlockingTask| -> BoxedBlockingTask {
    ///             Box::new(move || {
    ///                 println!("running blocking task");
    ///                 task();
    ///             })
    ///         },
    ///     )
    ///     .build();
    ///
    /// let result = spawner.spawn_blocking(|| 42).await;
    /// assert_eq!(result, 42);
    /// # }
    /// # #[cfg(not(feature = "tokio"))]
    /// # fn main() {}
    /// ```
    pub fn layer<FL, BL>(self, future_layer: FL, blocking_layer: BL) -> CustomSpawnerBuilder<impl SpawnCustom + Clone>
    where
        FL: Fn(BoxedFuture) -> BoxedFuture + Clone + Send + Sync + 'static,
        BL: Fn(BoxedBlockingTask) -> BoxedBlockingTask + Clone + Send + Sync + 'static,
    {
        CustomSpawnerBuilder {
            spawner: Layered {
                future_layer,
                blocking_layer,
                inner: self.spawner,
            },
            name: self.name,
        }
    }

    /// Builds the [`Spawner`] from the composed layers and base spawner.
    pub fn build(self) -> Spawner {
        Spawner::new_custom(self.name, self.spawner)
    }
}

#[expect(clippy::missing_fields_in_debug, reason = "spawner is opaque and not useful in debug output")]
impl<S> Debug for CustomSpawnerBuilder<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("CustomSpawnerBuilder");
        s.field("name", &self.name);
        s.finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    /// Mock spawner whose relocate sets a flag, so mutation tests catch no-ops.
    #[derive(Clone)]
    struct TrackingSpawner {
        relocated: &'static AtomicBool,
    }

    impl ThreadAware for TrackingSpawner {
        fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {
            self.relocated.store(true, Ordering::SeqCst);
        }
    }

    impl SpawnCustom for TrackingSpawner {
        fn spawn(&self, _task: BoxedFuture) {}

        fn spawn_anywhere(&self, mut task: Box<dyn ThreadAwareAsyncFnOnce<()>>) {
            let affinities = thread_aware::affinity::pinned_affinities(&[2]);
            task.relocate(Some(affinities[0]), affinities[1]);
        }

        fn spawn_blocking(&self, _task: BoxedBlockingTask) {}
    }

    /// Minimal async task for covering `spawn_anywhere`.
    struct NoopTask;
    impl ThreadAware for NoopTask {
        fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {}
    }
    impl ThreadAwareAsyncFnOnce<()> for NoopTask {
        fn call_once(self: Box<Self>) -> thread_aware::closure::BoxFuture<'static, ()> {
            Box::pin(async {})
        }
    }

    #[test]
    fn layered_relocate_forwards_to_inner() {
        static RELOCATED: AtomicBool = AtomicBool::new(false);
        static BLOCKING_LAYER_RAN: AtomicBool = AtomicBool::new(false);

        let affinities = thread_aware::affinity::pinned_affinities(&[2]);
        let mut layered = Layered {
            future_layer: |task: BoxedFuture| -> BoxedFuture { task },
            blocking_layer: |task: BoxedBlockingTask| -> BoxedBlockingTask {
                BLOCKING_LAYER_RAN.store(true, Ordering::SeqCst);
                task
            },
            inner: TrackingSpawner { relocated: &RELOCATED },
        };

        layered.relocate(Some(affinities[0]), affinities[1]);
        assert!(RELOCATED.load(Ordering::SeqCst), "Layered must forward relocate to inner");

        // Exercise spawn + spawn_anywhere + spawn_blocking + layer closures + NoopTask::call_once
        layered.spawn(Box::pin(async {}));
        layered.spawn_anywhere(Box::new(NoopTask));
        layered.spawn_blocking(Box::new(|| {}));
        assert!(
            BLOCKING_LAYER_RAN.load(Ordering::SeqCst),
            "blocking_layer must run on spawn_blocking"
        );

        let covered = (layered.future_layer)(Box::pin(async {}));
        futures::executor::block_on(covered);
        futures::executor::block_on(Box::new(NoopTask).call_once());
    }

    #[test]
    fn layered_task_relocate_forwards_to_inner() {
        static RELOCATED: AtomicBool = AtomicBool::new(false);

        #[derive(Clone)]
        struct Tracker(&'static AtomicBool);

        impl ThreadAware for Tracker {
            fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        impl ThreadAwareAsyncFnOnce<()> for Tracker {
            fn call_once(self: Box<Self>) -> thread_aware::closure::BoxFuture<'static, ()> {
                Box::pin(async {})
            }
        }

        let affinities = thread_aware::affinity::pinned_affinities(&[2]);
        let mut task = LayeredTask {
            task: Box::new(Tracker(&RELOCATED)),
            layer: |task: BoxedFuture| -> BoxedFuture { task },
        };

        task.relocate(Some(affinities[0]), affinities[1]);
        assert!(RELOCATED.load(Ordering::SeqCst), "LayeredTask must forward relocate to inner task");

        // Exercise layer closure to cover helper code
        let covered = (task.layer)(Box::pin(async {}));
        futures::executor::block_on(covered);

        // Exercise call_once (consumes the task)
        let fut = task.task.call_once();
        futures::executor::block_on(fut);
    }
}
