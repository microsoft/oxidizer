// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`CustomSpawnerBuilder`] for composing layered spawn functions.

use std::fmt::Debug;

use crate::Spawner;
use crate::custom::BoxedFuture;

/// A typestate builder for constructing a [`Spawner`] with layered future
/// transformations.
///
/// # Design
///
/// Layers compose statically via generics — no `Arc` or `Box<dyn Fn>` is
/// allocated until the final [`build()`](Self::build) step, which produces a
/// single `Arc`-wrapped closure for the [`Spawner`].
///
/// The builder works bottom-to-top: start by choosing a base spawn function
/// with [`tokio()`](Self::tokio) or [`custom()`](Self::custom), then stack
/// layers with [`layer()`](Self::layer), and finalize with
/// [`build()`](Self::build).
///
/// # Examples
///
/// ```rust
/// use anyspawn::{BoxedFuture, CustomSpawnerBuilder};
///
/// # #[tokio::main]
/// # async fn main() {
/// let spawner = CustomSpawnerBuilder::tokio()
///     .layer(|fut: BoxedFuture, spawn: &dyn Fn(BoxedFuture)| {
///         spawn(Box::pin(async move {
///             println!("before");
///             fut.await;
///             println!("after");
///         }));
///     })
///     .build();
///
/// let result = spawner.spawn(async { 42 }).await;
/// assert_eq!(result, 42);
/// # }
/// ```
pub struct CustomSpawnerBuilder<S> {
    spawn_fn: S,
}

impl CustomSpawnerBuilder<()> {
    /// Creates a builder using Tokio as the base spawn function.
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
    #[cfg_attr(docsrs, doc(cfg(all(feature = "tokio", feature = "custom"))))]
    pub fn tokio() -> CustomSpawnerBuilder<impl Fn(BoxedFuture) + Send + Sync + 'static> {
        CustomSpawnerBuilder {
            spawn_fn: |fut: BoxedFuture| {
                ::tokio::spawn(fut);
            },
        }
    }

    /// Creates a builder with a custom base spawn function.
    ///
    /// The closure receives a [`BoxedFuture`] and is responsible for spawning it
    /// on the target runtime.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use anyspawn::{BoxedFuture, CustomSpawnerBuilder};
    ///
    /// let spawner = CustomSpawnerBuilder::custom(|fut: BoxedFuture| {
    ///     std::thread::spawn(move || futures::executor::block_on(fut));
    /// })
    /// .build();
    /// ```
    pub fn custom<S>(spawn: S) -> CustomSpawnerBuilder<S>
    where
        S: Fn(BoxedFuture) + Send + Sync + 'static,
    {
        CustomSpawnerBuilder { spawn_fn: spawn }
    }
}

impl<S> CustomSpawnerBuilder<S>
where
    S: Fn(BoxedFuture) + Send + Sync + 'static,
{
    /// Adds a layer that intercepts futures before they reach the inner spawn
    /// function.
    ///
    /// The layer closure receives:
    /// - The [`BoxedFuture`] to be spawned.
    /// - A reference to the inner spawn function (previous layer or base).
    ///
    /// Layers compose from outside in: the last added layer runs first when
    /// [`Spawner::spawn`] is called.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anyspawn::{BoxedFuture, CustomSpawnerBuilder};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let spawner = CustomSpawnerBuilder::tokio()
    ///     .layer(|fut: BoxedFuture, spawn: &dyn Fn(BoxedFuture)| {
    ///         // Wrap the future with tracing, context propagation, etc.
    ///         spawn(Box::pin(async move {
    ///             println!("task starting");
    ///             fut.await;
    ///         }));
    ///     })
    ///     .build();
    /// # let _ = spawner;
    /// # }
    /// ```
    pub fn layer<L>(
        self,
        layer_fn: L,
    ) -> CustomSpawnerBuilder<impl Fn(BoxedFuture) + Send + Sync + 'static>
    where
        L: Fn(BoxedFuture, &dyn Fn(BoxedFuture)) + Send + Sync + 'static,
    {
        let inner = self.spawn_fn;
        CustomSpawnerBuilder {
            spawn_fn: move |fut: BoxedFuture| {
                layer_fn(fut, &inner);
            },
        }
    }

    /// Builds the [`Spawner`] from the composed layers and spawn function.
    ///
    /// This is the only step that boxes the spawn function: the fully composed
    /// closure is wrapped in an `Arc<dyn Fn>` for use by the [`Spawner`].
    pub fn build(self) -> Spawner {
        Spawner::new_custom(self.spawn_fn)
    }
}

impl<S> Debug for CustomSpawnerBuilder<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomSpawnerBuilder").finish_non_exhaustive()
    }
}
