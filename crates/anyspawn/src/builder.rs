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
/// Layers compose statically via generics - no `Arc` or `Box<dyn Fn>` is
/// allocated until the final [`build()`](Self::build) step, which produces a
/// single `Arc`-wrapped closure for the [`Spawner`].
///
/// The builder works bottom-to-top: start by choosing a base spawn function
/// with [`tokio()`](Self::tokio) or [`custom()`](Self::custom), then stack
/// layers with [`layer()`](Self::layer), and finalize with
/// [`build()`](Self::build).
///
/// Every constructor and layer requires a name so that the resulting
/// [`Spawner`] can describe its composition in [`Debug`] output.
///
/// # Examples
///
/// ```rust
/// use anyspawn::{BoxedFuture, CustomSpawnerBuilder};
///
/// # #[tokio::main]
/// # async fn main() {
/// let spawner = CustomSpawnerBuilder::tokio()
///     .layer("logging", |fut: BoxedFuture, spawn: &dyn Fn(BoxedFuture)| {
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
    name: &'static str,
    layer_names: Vec<Box<str>>,
}

impl CustomSpawnerBuilder<()> {
    /// Creates a builder using Tokio as the base spawn function.
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
    #[cfg_attr(docsrs, doc(cfg(all(feature = "tokio", feature = "custom"))))]
    #[must_use]
    pub fn tokio() -> CustomSpawnerBuilder<impl Fn(BoxedFuture) + Send + Sync + 'static> {
        CustomSpawnerBuilder {
            spawn_fn: |fut: BoxedFuture| {
                ::tokio::spawn(fut);
            },
            name: "tokio",
            layer_names: Vec::new(),
        }
    }

    /// Creates a builder with a custom base spawn function.
    ///
    /// The `name` identifies this spawner in [`Debug`] output.
    /// The closure receives a [`BoxedFuture`] and is responsible for spawning it
    /// on the target runtime.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use anyspawn::{BoxedFuture, CustomSpawnerBuilder};
    ///
    /// let spawner = CustomSpawnerBuilder::custom("threadpool", |fut: BoxedFuture| {
    ///     std::thread::spawn(move || futures::executor::block_on(fut));
    /// })
    /// .build();
    /// ```
    pub fn custom<S>(name: &'static str, spawn: S) -> CustomSpawnerBuilder<S>
    where
        S: Fn(BoxedFuture) + Send + Sync + 'static,
    {
        CustomSpawnerBuilder {
            spawn_fn: spawn,
            name,
            layer_names: Vec::new(),
        }
    }
}

impl<S> CustomSpawnerBuilder<S>
where
    S: Fn(BoxedFuture) + Send + Sync + 'static,
{
    /// Adds a named layer that intercepts futures before they reach the inner
    /// spawn function.
    ///
    /// The `name` identifies this layer in [`Debug`] output.
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
    ///     .layer("tracing", |fut: BoxedFuture, spawn: &dyn Fn(BoxedFuture)| {
    ///         spawn(Box::pin(async move {
    ///             println!("task starting");
    ///             fut.await;
    ///         }));
    ///     })
    ///     .build();
    /// # let _ = spawner;
    /// # }
    /// ```
    pub fn layer<L>(self, name: impl AsRef<str>, layer_fn: L) -> CustomSpawnerBuilder<impl Fn(BoxedFuture) + Send + Sync + 'static>
    where
        L: Fn(BoxedFuture, &dyn Fn(BoxedFuture)) + Send + Sync + 'static,
    {
        let inner = self.spawn_fn;
        let mut layer_names = self.layer_names;
        layer_names.push(Box::from(name.as_ref()));
        CustomSpawnerBuilder {
            spawn_fn: move |fut: BoxedFuture| {
                layer_fn(fut, &inner);
            },
            name: self.name,
            layer_names,
        }
    }

    /// Builds the [`Spawner`] from the composed layers and spawn function.
    ///
    /// This is the only step that boxes the spawn function: the fully composed
    /// closure is wrapped in an `Arc<dyn Fn>` for use by the [`Spawner`].
    pub fn build(self) -> Spawner {
        Spawner::new_with_layers(self.name, self.spawn_fn, self.layer_names.into())
    }
}

#[expect(clippy::missing_fields_in_debug, reason = "spawn_fn is a closure and not useful in debug output")]
impl<S> Debug for CustomSpawnerBuilder<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("CustomSpawnerBuilder");
        s.field("name", &self.name);
        if !self.layer_names.is_empty() {
            s.field("layers", &self.layer_names);
        }
        s.finish()
    }
}
