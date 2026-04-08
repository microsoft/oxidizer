use crate::ResolveFrom;
use crate::resolver_store::ResolverStore;

/// Terminator for a heterogeneous dependency list.
pub struct ResolutionDepsEnd;

/// A node in a heterogeneous dependency list, pairing a head type `H` with a tail `T`.
pub struct ResolutionDepsNode<H, T>(pub H, pub T);

/// A heterogeneous list of dependencies that can be resolved from a [`ResolverStore`].
pub trait ResolutionDeps<T: 'static>: Send + Sync + 'static {
    /// The resolved form of this dependency list, holding references to each dependency.
    type Resolved<'a>
    where
        Self: 'a,
        T: 'a;

    /// Ensures every dependency in the list is resolved in the store.
    fn ensure<S: ResolverStore<T>>(store: &mut S);

    /// Returns references to already-resolved dependencies without triggering resolution.
    fn get_private<S: ResolverStore<T>>(store: &S) -> Self::Resolved<'_>;

    /// Ensures all dependencies are resolved, then returns references to them.
    fn get<S: ResolverStore<T>>(store: &mut S) -> Self::Resolved<'_> {
        Self::ensure(store);
        Self::get_private(store)
    }
}

impl<T: 'static> ResolutionDeps<T> for ResolutionDepsEnd {
    type Resolved<'a>
        = ResolutionDepsEnd
    where
        Self: 'a,
        T: 'a;

    fn ensure<S: ResolverStore<T>>(_store: &mut S) {}

    fn get_private<S: ResolverStore<T>>(_store: &S) -> Self::Resolved<'_> {
        ResolutionDepsEnd
    }
}

impl<T, H, Rest> ResolutionDeps<T> for ResolutionDepsNode<H, Rest>
where
    H: ResolveFrom<T>,
    Rest: ResolutionDeps<T>,
    T: Send + Sync + 'static,
{
    type Resolved<'a>
        = ResolutionDepsNode<&'a H, Rest::Resolved<'a>>
    where
        Self: 'a,
        T: 'a;
    fn get_private<S: ResolverStore<T>>(store: &S) -> Self::Resolved<'_> {
        let tail = Rest::get_private(store);
        let head = store.lookup::<H>().expect("ensure must have been called before get_private");
        ResolutionDepsNode(head, tail)
    }

    fn ensure<S: ResolverStore<T>>(store: &mut S) {
        store.resolve::<H>();
        Rest::ensure(store);
    }
}
