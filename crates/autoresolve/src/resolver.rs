use std::any::TypeId;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::base_type::BaseType;
use crate::path_cache::PathCache;
use crate::path_stack::PathStack;
use crate::resolve_deps::ResolutionDeps;
use crate::resolve_from::ResolveFrom;
use crate::resolve_output::ResolveOutput;

/// A resolver that lazily constructs types from their dependencies.
///
/// Internally backed by an `Arc<PathCache>` so that resolved values are
/// visible to child scopes created via [`scoped()`](Resolver::scoped).
///
/// ## Tier convention
///
/// Each resolver has a `level`: 0 for the root resolver, incremented by one
/// for every `scoped()` call. A resolved value's *tier* is the level of the
/// resolver whose cache holds it. The placement tier of a newly constructed
/// value equals the maximum tier across its dependencies — a value cannot
/// live at a level deeper than any of its inputs (otherwise the resolver at
/// that shallower level could not see the dependency).
///
/// A leaf type with no dependencies promotes to the root (tier 0), which
/// preserves the historical "promote leaves to the deepest ancestor" behavior.
pub struct Resolver<T> {
    /// Local path-keyed cache.
    types: Arc<PathCache>,
    /// Ancestor caches in *root-first* order. `ancestors[0]` is the root
    /// resolver's cache; `ancestors[level - 1]` is the immediate parent's
    /// cache. Empty for the root resolver.
    ancestors: Vec<Arc<PathCache>>,
    /// This resolver's tier (0 = root, +1 per `scoped()` call). Equal to
    /// `ancestors.len()` by construction.
    level: usize,
    base: PhantomData<T>,
}

impl<T> std::fmt::Debug for Resolver<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resolver")
            .field("level", &self.level)
            .field("ancestors", &self.ancestors.len())
            .finish_non_exhaustive()
    }
}

impl<T: Send + Sync + 'static> Resolver<T> {
    /// Creates a resolver from a base type that implements [`BaseType`].
    ///
    /// The base struct's fields are automatically inserted as root types. Use
    /// the `#[base]` proc macro to generate the [`BaseType`] implementation.
    pub fn new(base: T) -> Self
    where
        T: BaseType<Parent = ()>,
    {
        let mut resolver = Self::new_empty();
        base.insert_into(&mut resolver);
        resolver
    }

    /// Creates an empty resolver with no pre-inserted types.
    #[must_use]
    pub fn new_empty() -> Self {
        Self {
            types: Arc::new(PathCache::new()),
            ancestors: Vec::new(),
            level: 0,
            base: PhantomData,
        }
    }

    /// Pre-inserts a value into the resolver's local cache at the
    /// single-element path `[TypeId::of::<V>()]`.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "&mut self reflects logical ownership; PathCache uses interior mutability"
    )]
    pub fn insert<V: Send + Sync + 'static>(&mut self, value: V) {
        let key = vec![TypeId::of::<V>()];
        let _ = self.types.get_or_insert(key, value);
    }

    /// Resolves a type, lazily constructing it from its dependencies if needed.
    pub fn ensure<O>(&mut self) -> Arc<O>
    where
        O: ResolveFrom<T>,
    {
        self.get::<O>()
    }

    /// Resolves a type, lazily constructing it from its dependencies if needed.
    pub fn get<O>(&mut self) -> Arc<O>
    where
        O: ResolveFrom<T>,
    {
        let root = PathStack::root();
        self.resolve::<O>(&root).value
    }

    /// Returns a handle to an already-resolved type, or `None` if it has not
    /// been resolved.
    #[must_use]
    pub fn try_get<O>(&self) -> Option<Arc<O>>
    where
        O: ResolveFrom<T>,
    {
        self.lookup_classical::<O>().map(|(v, _)| v)
    }

    /// Creates a child resolver that inherits types from this resolver.
    ///
    /// The scoped base struct's fields are automatically inserted as root types.
    /// Types already resolved in this resolver (and its ancestors) are visible
    /// to the child. New types resolved by the child are placed at the maximum
    /// tier across their dependencies — promoting them to a shallower
    /// ancestor when possible, preserving the historical pooling behavior.
    pub fn scoped<S>(&self, roots: S) -> Resolver<S>
    where
        S: BaseType<Parent = T>,
    {
        // Ancestors are stored root-first; the new child appends `self.types`
        // at the end (closest to itself).
        let mut ancestors = Vec::with_capacity(self.ancestors.len() + 1);
        ancestors.extend(self.ancestors.iter().map(Arc::clone));
        ancestors.push(Arc::clone(&self.types));

        let mut resolver = Resolver::<S> {
            types: Arc::new(PathCache::new()),
            ancestors,
            level: self.level + 1,
            base: PhantomData,
        };
        roots.insert_into(&mut resolver);
        resolver
    }

    /// Returns a reference to the cache at the given tier.
    fn cache_at(&self, tier: usize) -> &Arc<PathCache> {
        if tier == self.level { &self.types } else { &self.ancestors[tier] }
    }

    /// Looks up the value of type `O` cached at the single-element path
    /// `[TypeId::of::<O>()]`, walking from the closest cache (self) outward,
    /// and reports the tier at which it lives.
    ///
    /// In phase 2 this is the only cache key shape ever used.
    fn lookup_classical<O: Send + Sync + 'static>(&self) -> Option<(Arc<O>, usize)> {
        let key = [TypeId::of::<O>()];
        if let Some(v) = self.types.try_get::<O>(&key) {
            return Some((v, self.level));
        }
        // Ancestors are root-first: ancestors[0] is the root (tier 0),
        // ancestors[i] is at tier `i`. Walk closest-first (descending index).
        for (i, cache) in self.ancestors.iter().enumerate().rev() {
            if let Some(v) = cache.try_get::<O>(&key) {
                return Some((v, i));
            }
        }
        None
    }

    /// Resolves a type along the current `path`, lazily constructing it from
    /// its dependencies if needed and returning a handle plus its placement
    /// tier.
    ///
    /// In phase 2, the path is plumbed through but the cache always uses the
    /// single-element key `[TypeId::of::<O>()]`. Phase 3+ will use `path` to
    /// build longer cache keys for scoped overrides.
    pub fn resolve<O>(&mut self, path: &PathStack<'_>) -> ResolveOutput<O>
    where
        O: ResolveFrom<T>,
    {
        // Fast path: already cached locally or in an ancestor. The returned
        // `Arc` is owned, so no borrow on `self` outlives this branch.
        if let Some((value, tier)) = self.lookup_classical::<O>() {
            return ResolveOutput::new(value, tier);
        }

        // Slow path: ensure all dependencies, then construct.
        let child_path = path.push(TypeId::of::<O>());
        let deps_tier = <<O as ResolveFrom<T>>::Inputs as ResolutionDeps<T>>::ensure_all(self, &child_path);
        let inputs = <<O as ResolveFrom<T>>::Inputs as ResolutionDeps<T>>::collect(self, &child_path);
        let value = O::new(inputs);

        // Place the value at `deps_tier` — the deepest tier whose cache
        // contains all inputs (the maximum tier across all dependencies).
        let placement = Arc::clone(self.cache_at(deps_tier));
        let key = vec![TypeId::of::<O>()];
        let stored = placement.get_or_insert(key, value);
        ResolveOutput::new(stored, deps_tier)
    }

    /// Internal accessor used by [`ResolutionDeps::collect`] to retrieve an
    /// already-resolved dependency without triggering further resolution.
    pub(crate) fn lookup_for_collect<O: Send + Sync + 'static>(&self, _path: &PathStack<'_>) -> Option<Arc<O>> {
        self.lookup_classical::<O>().map(|(v, _)| v)
    }
}
