//! Override registration: the `provide()` builder.
//!
//! `provide(value)` returns a [`ProvideBuilder`] that begins as
//! [`Unscoped`](crate::Unscoped). Each `.when_injected_in::<T>()` extends the
//! injection chain at the type level (and is validated against the
//! [`DependencyOf`](crate::DependencyOf) marker emitted by `#[resolvable]`).
//! When the builder is dropped the registration commits: pre-allocated empty
//! slots for every prefix of the chain, and a filled slot at the leaf.
//!
//! Branched registration via `either` / `or` lives in the [`branching`]
//! submodule.

mod branching;

pub use branching::{BranchBuilder, Branched};

use core::any::TypeId;
use core::marker::PhantomData;
use std::sync::Arc;

use crate::dependency_of::DependencyOf;
use crate::path_cache::PathCache;
use crate::provide_path::{Scoped, Unscoped};

/// Compile-time projection of the *head* of the override chain.
///
/// Implemented for [`Unscoped`] (head is the originally provided type
/// `Initial`) and for [`Scoped`] (head is the most recently added consumer).
pub trait PathHead<Initial: 'static> {
    /// The type at the head of the chain — the next link must declare this
    /// type as a `DependencyOf<NewTarget>`.
    type Head: 'static;
}

impl<Initial: 'static> PathHead<Initial> for Unscoped {
    type Head = Initial;
}

impl<Initial: 'static, I: 'static, Rest> PathHead<Initial> for Scoped<I, Rest> {
    type Head = I;
}

/// Materializes the override chain as a root-first sequence of `TypeId`s.
///
/// `Initial` is the originally provided value type (becomes the *last* entry
/// of the path). The outermost (newest) `Scoped` wrapper becomes the *first*
/// entry — the root of the chain.
pub trait CollectPath<Initial: 'static> {
    /// Pushes the chain's `TypeId`s onto `buf` in root-first order.
    fn collect(buf: &mut Vec<TypeId>);
}

impl<Initial: 'static> CollectPath<Initial> for Unscoped {
    fn collect(buf: &mut Vec<TypeId>) {
        buf.push(TypeId::of::<Initial>());
    }
}

impl<Initial: 'static, I: 'static, Rest: CollectPath<Initial>> CollectPath<Initial> for Scoped<I, Rest> {
    fn collect(buf: &mut Vec<TypeId>) {
        buf.push(TypeId::of::<I>());
        Rest::collect(buf);
    }
}

/// Fluent builder returned by [`Resolver::provide`](crate::Resolver::provide).
///
/// Each `.when_injected_in::<T>()` extends the chain. Drop commits the
/// registration to the resolver's local path cache — typically by writing the
/// expression as a statement (`resolver.provide(v).when_injected_in::<T>();`).
///
/// `'r` borrows the cache from the originating `Resolver`. `Initial` is the
/// originally provided value type. `Path` is the type-level chain tag,
/// starting at [`Unscoped`] and growing with each call.
#[derive(Debug)]
pub struct ProvideBuilder<'r, Initial: Send + Sync + 'static, Path = Unscoped>
where
    Path: CollectPath<Initial>,
{
    pub(super) cache: &'r Arc<PathCache>,
    /// `Some(value)` until commit on drop.
    pub(super) value: Option<Initial>,
    _path: PhantomData<fn() -> Path>,
}

impl<'r, Initial: Send + Sync + 'static, Path> ProvideBuilder<'r, Initial, Path>
where
    Path: CollectPath<Initial>,
{
    /// Internal constructor used by `Resolver::provide`.
    pub(crate) fn new(cache: &'r Arc<PathCache>, value: Initial) -> ProvideBuilder<'r, Initial, Unscoped> {
        ProvideBuilder {
            cache,
            value: Some(value),
            _path: PhantomData,
        }
    }

    /// Extends the override chain with a new consumer link.
    ///
    /// The current head of the chain must be a declared
    /// [`DependencyOf<Target>`], i.e. the `#[resolvable]` macro must have
    /// emitted that impl based on `Target::new` taking `&CurrentHead`.
    pub fn when_injected_in<Target>(self) -> ProvideBuilder<'r, Initial, Scoped<Target, Path>>
    where
        Target: 'static,
        Path: PathHead<Initial>,
        <Path as PathHead<Initial>>::Head: DependencyOf<Target>,
    {
        // Move the value into the new builder; the old `self` is forgotten so
        // its `Drop` does not commit a partial chain.
        let mut me = core::mem::ManuallyDrop::new(self);
        let value = me.value.take();
        let cache = me.cache;
        ProvideBuilder {
            cache,
            value,
            _path: PhantomData,
        }
    }

    /// Opens a branched section: the same value applies to multiple
    /// alternative path tails. Add more alternatives with [`Branched::or`].
    ///
    /// The closure receives a [`BranchBuilder`] rooted at the *current*
    /// chain. Returning it unchanged (`|x| x`) registers the current chain
    /// itself as one of the alternatives.
    pub fn either<F, B>(self, f: F) -> Branched<'r, Initial, Path>
    where
        F: FnOnce(BranchBuilder<Initial, Path>) -> BranchBuilder<Initial, B>,
        B: CollectPath<Initial>,
    {
        // Suppress the linear `Drop` and steal the value/cache.
        let mut me = core::mem::ManuallyDrop::new(self);
        let value = me.value.take();
        let cache = me.cache;

        let _branch: BranchBuilder<Initial, B> = f(BranchBuilder::new());
        let mut leaf: Vec<TypeId> = Vec::new();
        B::collect(&mut leaf);

        Branched::from_first_branch(cache, value, leaf)
    }
}

impl<Initial: Send + Sync + 'static, Path> Drop for ProvideBuilder<'_, Initial, Path>
where
    Path: CollectPath<Initial>,
{
    fn drop(&mut self) {
        let Some(value) = self.value.take() else {
            return;
        };

        // Materialize the leaf path (root-first).
        let mut leaf: Vec<TypeId> = Vec::new();
        Path::collect(&mut leaf);
        debug_assert!(!leaf.is_empty());

        // Pre-create empty slots for every strict prefix.
        for end in 1..leaf.len() {
            let _ = self.cache.get_or_create_slot(leaf[..end].to_vec());
        }

        // Fill the leaf slot.
        let _ = self.cache.get_or_insert(leaf, value);
    }
}
