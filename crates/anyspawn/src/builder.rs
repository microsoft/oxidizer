// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`CustomSpawnerBuilder`] for composing layered spawners.

use std::fmt::Debug;

use crate::Spawner;
use crate::custom::{BoxedFuture, SpawnCustom};
use thread_aware::ThreadAware;
use thread_aware::affinity::Affinity;
use thread_aware::closure::ThreadAwareAsyncFnOnce;

/// Internal composition of a layer closure wrapping an inner [`SpawnCustom`].
///
/// The closure transforms futures before they are forwarded to the inner
/// spawner. During relocation only the inner spawner is notified; closures
/// are expected to be stateless (or capture only `Arc`-based state that
/// does not need relocation).
struct Layered<F, S> {
    layer: F,
    inner: S,
}

impl<F: Clone, S: Clone> Clone for Layered<F, S> {
    fn clone(&self) -> Self {
        Self {
            layer: self.layer.clone(),
            inner: self.inner.clone(),
        }
    }
}

impl<F: Send, S: ThreadAware> ThreadAware for Layered<F, S> {
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        self.inner.relocate(source, destination);
    }
}

impl<F, S> SpawnCustom for Layered<F, S>
where
    F: Fn(BoxedFuture) -> BoxedFuture + Clone + Send + Sync + 'static,
    S: SpawnCustom + Clone,
{
    fn spawn(&self, task: BoxedFuture) {
        self.inner.spawn((self.layer)(task));
    }

    fn spawn_anywhere(&self, task: Box<dyn ThreadAwareAsyncFnOnce<()>>) {
        // Wrap the original task so the inner spawner can relocate it before
        // call_once(). The layer is applied lazily inside call_once() so that
        // the captured ThreadAware data is relocated first.
        let layered = Box::new(LayeredTask {
            task,
            layer: self.layer.clone(),
        });
        self.inner.spawn_anywhere(layered);
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
}

/// A builder for constructing a [`Spawner`] with layered middleware.
///
/// Each layer is a closure `Fn(BoxedFuture) -> BoxedFuture` that transforms
/// the spawned future before it reaches the inner spawner. Layers compose
/// from outside in: the last added layer runs first when
/// [`Spawner::spawn`] is called.
///
/// # Design
///
/// Use [`new`](Self::new) with any [`SpawnCustom`] base, or the convenience
/// [`tokio()`](Self::tokio) constructor. Stack middleware with
/// [`layer()`](Self::layer) and finalize with [`build()`](Self::build).
///
/// # Note
///
/// [`Spawner::new_tokio`] uses a more efficient code path with native Tokio
/// `JoinHandle`s. The builder's [`tokio()`](Self::tokio) constructor goes
/// through the custom spawner path (using a oneshot channel for join handles),
/// which is necessary to support layers but slightly less efficient for
/// unlayered use.
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "tokio")]
/// # #[tokio::main]
/// # async fn main() {
/// use anyspawn::{BoxedFuture, CustomSpawnerBuilder};
///
/// let spawner = CustomSpawnerBuilder::tokio()
///     .layer(|task: BoxedFuture| -> BoxedFuture {
///         println!("spawning task");
///         task
///     })
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

    /// Adds a layer that transforms futures before they reach the inner
    /// spawner.
    ///
    /// The closure receives a [`BoxedFuture`] and must return a
    /// [`BoxedFuture`]. It is invoked for both [`Spawner::spawn`] and
    /// [`Spawner::spawn_anywhere`].
    ///
    /// Layers compose from outside in: the last added layer runs first.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "tokio")]
    /// # #[tokio::main]
    /// # async fn main() {
    /// use anyspawn::{BoxedFuture, CustomSpawnerBuilder};
    ///
    /// let spawner = CustomSpawnerBuilder::tokio()
    ///     .layer(|task: BoxedFuture| -> BoxedFuture { task })
    ///     .build();
    /// # let _ = spawner;
    /// # }
    /// # #[cfg(not(feature = "tokio"))]
    /// # fn main() {}
    /// ```
    pub fn layer<F>(self, layer: F) -> CustomSpawnerBuilder<impl SpawnCustom + Clone>
    where
        F: Fn(BoxedFuture) -> BoxedFuture + Clone + Send + Sync + 'static,
    {
        CustomSpawnerBuilder {
            spawner: Layered {
                layer,
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

        let affinities = thread_aware::affinity::pinned_affinities(&[2]);
        let mut layered = Layered {
            layer: |task: BoxedFuture| -> BoxedFuture { task },
            inner: TrackingSpawner { relocated: &RELOCATED },
        };

        layered.relocate(Some(affinities[0]), affinities[1]);
        assert!(RELOCATED.load(Ordering::SeqCst), "Layered must forward relocate to inner");

        // Exercise spawn + spawn_anywhere + layer closure + NoopTask::call_once
        layered.inner.spawn(Box::pin(async {}));
        layered.inner.spawn_anywhere(Box::new(NoopTask));
        let covered = (layered.layer)(Box::pin(async {}));
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
