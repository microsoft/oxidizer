use crate::resolve_from::ResolveFrom;

/// Abstraction over resolver storage used internally by
/// [`ResolutionDeps`](crate::ResolutionDeps) to recursively resolve dependency
/// graphs. Users typically interact with [`Resolver`](crate::Resolver) whose
/// inherent methods delegate to this trait.
pub trait ResolverStore<T: 'static> {
    /// Resolves a type, lazily constructing it from its dependencies if not yet present.
    fn resolve<O: ResolveFrom<T>>(&mut self) -> &O;

    /// Looks up an already-resolved type without triggering resolution.
    fn lookup<O: Send + Sync + 'static>(&self) -> Option<&O>;

    /// Stores a pre-constructed value into the resolver.
    fn store_value<O: Send + Sync + 'static>(&mut self, value: O);
}
