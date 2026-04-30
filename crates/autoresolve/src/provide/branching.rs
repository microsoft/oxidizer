//! Branching override registration via `either` / `or`.
//!
//! [`ProvideBuilder::either`](super::ProvideBuilder::either) opens a branched
//! section in which the same provided value applies to multiple alternative
//! path tails. Each branch's leaf path gets its own pre-allocated slot, and
//! all leaf slots are filled with the *same* shared `Arc` of the value — so
//! every branch observes the same override instance.

use core::any::TypeId;
use core::marker::PhantomData;
use std::sync::Arc;

use super::{CollectPath, PathHead};
use crate::dependency_of::DependencyOf;
use crate::path_cache::PathCache;
use crate::provide_path::{Scoped, Unscoped};

/// Type-only builder passed into [`ProvideBuilder::either`](super::ProvideBuilder::either)
/// and [`Branched::or`] closures.
///
/// Carries the type-level injection chain forward exactly like
/// [`ProvideBuilder`](super::ProvideBuilder), but holds no value and never commits — its purpose is
/// to materialize a single branch's leaf path at the type level. The
/// outer [`Branched`] aggregator collects every branch's leaf and commits
/// them together on drop.
#[derive(Debug)]
pub struct BranchBuilder<Initial: 'static, Path = Unscoped> {
    _initial: PhantomData<fn() -> Initial>,
    _path: PhantomData<fn() -> Path>,
}

impl<Initial: 'static, Path> BranchBuilder<Initial, Path> {
    pub(super) fn new() -> Self {
        Self {
            _initial: PhantomData,
            _path: PhantomData,
        }
    }

    /// Extends this branch's chain. Same compile-time validation as
    /// [`ProvideBuilder::when_injected_in`](super::ProvideBuilder::when_injected_in).
    #[must_use]
    #[expect(
        clippy::unused_self,
        reason = "`self` carries the type-state of the chain being extended"
    )]
    pub fn when_injected_in<Target>(self) -> BranchBuilder<Initial, Scoped<Target, Path>>
    where
        Target: 'static,
        Path: PathHead<Initial>,
        <Path as PathHead<Initial>>::Head: DependencyOf<Target>,
    {
        BranchBuilder::new()
    }
}

/// Aggregator returned by [`ProvideBuilder::either`](super::ProvideBuilder::either).
///
/// Holds the value plus one leaf path per branch. On drop, every branch's
/// prefix slots are pre-allocated and every leaf slot is filled with the same
/// shared `Arc` of the value — guaranteeing all branches observe the same
/// override instance.
///
/// `Path` is the prefix type-level chain at the point of the original
/// `.either(...)` call; subsequent `.or(...)` closures receive a
/// `BranchBuilder<Initial, Path>` rooted at that same prefix.
#[derive(Debug)]
pub struct Branched<'r, Initial: Send + Sync + 'static, Path>
where
    Path: CollectPath<Initial>,
{
    pub(super) cache: &'r Arc<PathCache>,
    /// `Some(value)` until commit on drop.
    pub(super) value: Option<Initial>,
    /// One full leaf path per branch (root-first).
    pub(super) leaves: Vec<Vec<TypeId>>,
    pub(super) _path: PhantomData<fn() -> Path>,
}

impl<'r, Initial, Path> Branched<'r, Initial, Path>
where
    Initial: Send + Sync + 'static,
    Path: CollectPath<Initial>,
{
    /// Internal constructor used by [`ProvideBuilder::either`](super::ProvideBuilder::either).
    pub(super) fn from_first_branch(
        cache: &'r Arc<PathCache>,
        value: Option<Initial>,
        first_leaf: Vec<TypeId>,
    ) -> Self {
        Self {
            cache,
            value,
            leaves: vec![first_leaf],
            _path: PhantomData,
        }
    }

    /// Adds another alternative branch sharing the same value.
    ///
    /// The closure receives a [`BranchBuilder`] rooted at the same prefix as
    /// the original `.either(...)` call. Returning `x` unchanged registers
    /// the bare prefix path itself as one of the alternatives.
    #[expect(
        clippy::return_self_not_must_use,
        reason = "drop is the commit; #[must_use] would falsely warn on the intended `;` terminator"
    )]
    pub fn or<F, B>(mut self, f: F) -> Self
    where
        F: FnOnce(BranchBuilder<Initial, Path>) -> BranchBuilder<Initial, B>,
        B: CollectPath<Initial>,
    {
        let _branch: BranchBuilder<Initial, B> = f(BranchBuilder::new());
        let mut leaf: Vec<TypeId> = Vec::new();
        B::collect(&mut leaf);
        self.leaves.push(leaf);
        self
    }
}

impl<Initial, Path> Drop for Branched<'_, Initial, Path>
where
    Initial: Send + Sync + 'static,
    Path: CollectPath<Initial>,
{
    fn drop(&mut self) {
        let Some(value) = self.value.take() else {
            return;
        };
        let shared: Arc<dyn core::any::Any + Send + Sync> = Arc::new(value);

        for leaf in &self.leaves {
            debug_assert!(!leaf.is_empty());
            // Pre-create empty slots for every strict prefix.
            for end in 1..leaf.len() {
                let _ = self.cache.get_or_create_slot(leaf[..end].to_vec());
            }
            // Fill the leaf slot with the shared Arc. `OnceLock::set` is a
            // no-op if the slot is already filled (e.g. by an earlier branch
            // with an identical leaf path), so the first branch wins per
            // path.
            let slot = self.cache.get_or_create_slot(leaf.clone());
            let _ = slot.set(Arc::clone(&shared));
        }
    }
}
