use std::sync::Arc;

use crate::path_stack::PathStack;
use crate::resolve_from::ResolveFrom;
use crate::resolver::Resolver;

/// Terminator for a heterogeneous dependency list.
#[derive(Debug)]
pub struct ResolutionDepsEnd;

/// A node in a heterogeneous dependency list, pairing a head type `H` with a tail `T`.
#[derive(Debug)]
pub struct ResolutionDepsNode<H, T>(pub H, pub T);

/// A heterogeneous list of dependencies that can be resolved from a [`Resolver`].
pub trait ResolutionDeps<T: Send + Sync + 'static>: Send + Sync + 'static {
    /// The resolved form of this dependency list, holding `Arc` handles to
    /// each dependency.
    type Resolved;

    /// Ensures every dependency in the list is resolved in `store`.
    ///
    /// Returns the maximum tier across all resolved dependencies, used by the
    /// caller to decide where to place the value being constructed.
    fn ensure_all(store: &mut Resolver<T>, path: &PathStack<'_>) -> usize;

    /// Returns handles to already-resolved dependencies without triggering
    /// resolution. Must be preceded by [`ensure_all`](Self::ensure_all).
    fn collect(store: &Resolver<T>, path: &PathStack<'_>) -> Self::Resolved;
}

impl<T: Send + Sync + 'static> ResolutionDeps<T> for ResolutionDepsEnd {
    type Resolved = Self;

    fn ensure_all(_store: &mut Resolver<T>, _path: &PathStack<'_>) -> usize {
        // Empty deps list. Returning `0` (root tier) means "no upward
        // pressure": when `max`-reduced with sibling deps' tiers it is the
        // identity element, and for a true leaf with no other deps it means
        // promote all the way to the root resolver — preserving the
        // historical leaf-promotion behavior under the rule "placement tier =
        // max(dep tiers)".
        0
    }

    fn collect(_store: &Resolver<T>, _path: &PathStack<'_>) -> Self::Resolved {
        Self
    }
}

impl<T, H, Rest> ResolutionDeps<T> for ResolutionDepsNode<H, Rest>
where
    T: Send + Sync + 'static,
    H: ResolveFrom<T>,
    Rest: ResolutionDeps<T>,
{
    type Resolved = ResolutionDepsNode<Arc<H>, Rest::Resolved>;

    fn ensure_all(store: &mut Resolver<T>, path: &PathStack<'_>) -> usize {
        let head_tier = store.resolve::<H>(path).tier;
        let rest_tier = Rest::ensure_all(store, path);
        head_tier.max(rest_tier)
    }

    fn collect(store: &Resolver<T>, path: &PathStack<'_>) -> Self::Resolved {
        let tail = Rest::collect(store, path);
        let head = store.lookup_for_collect::<H>(path).expect("ensure_all must precede collect");
        ResolutionDepsNode(head, tail)
    }
}
