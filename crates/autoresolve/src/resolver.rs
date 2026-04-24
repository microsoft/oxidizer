use std::any::TypeId;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::base_type::BaseType;
use crate::path_cache::{PathCache, Slot};
use crate::path_stack::PathStack;
use crate::provide::ProvideBuilder;
use crate::provide_path::Unscoped;
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
/// resolver whose cache holds it.
///
/// - **Without overrides:** placement tier = max(dep tiers). A leaf type with
///   no dependencies promotes to the root (tier 0), which preserves the
///   historical "promote leaves to the deepest ancestor" behavior.
/// - **With an override-allocated slot:** placement tier = the level of the
///   resolver that owns the slot. Override-affected values therefore pin to
///   the override's home level.
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

    /// Begins a path-scoped value registration. See [`ProvideBuilder`] for
    /// the chain-building methods. The registration commits when the builder
    /// is dropped — typically at the end of the `provide(...)...;` statement.
    ///
    /// An unscoped `provide(value)` (no `when_injected_in` calls) is
    /// observationally equivalent to [`insert(value)`](Self::insert).
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "&mut self reflects logical ownership; PathCache uses interior mutability"
    )]
    pub fn provide<V: Send + Sync + 'static>(&mut self, value: V) -> ProvideBuilder<'_, V, Unscoped> {
        ProvideBuilder::<V, Unscoped>::new(&self.types, value)
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
    ///
    /// This is a path-free lookup: it only reports a value if one is cached
    /// at the single-element path `[TypeId::of::<O>()]` (i.e. the result of
    /// a classical resolution or an unscoped `provide`).
    #[must_use]
    pub fn try_get<O>(&self) -> Option<Arc<O>>
    where
        O: ResolveFrom<T>,
    {
        let key = [TypeId::of::<O>()];
        if let Some(v) = self.types.try_get::<O>(&key) {
            return Some(v);
        }
        for cache in self.ancestors.iter().rev() {
            if let Some(v) = cache.try_get::<O>(&key) {
                return Some(v);
            }
        }
        None
    }

    /// Creates a child resolver that inherits types from this resolver.
    ///
    /// The scoped base struct's fields are automatically inserted as root types.
    /// Types already resolved in this resolver (and its ancestors) are visible
    /// to the child. New types resolved by the child without a matching
    /// override slot are placed at the maximum tier across their dependencies
    /// — promoting them to a shallower ancestor when possible, preserving the
    /// historical pooling behavior.
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

    /// Walks self + ancestors closest-first and returns the best slot for
    /// resolving `target`-typed values along `key` (which must end in `target`).
    ///
    /// "Best" = longest cached path that is a suffix of `key`. Ties on length
    /// are broken by the *closest* tier (which is the first one we encounter
    /// when walking closest-first).
    fn find_best_slot(&self, key: &[TypeId], target: TypeId) -> Option<(Slot, usize)> {
        // (slot, path_len, tier). Seed with self (closest), then improve with
        // strictly-longer matches walking ancestors closest-first.
        let mut best: Option<(Slot, usize, usize)> =
            self.types.find_best_slot(key, target).map(|(slot, len)| (slot, len, self.level));
        for (i, cache) in self.ancestors.iter().enumerate().rev() {
            if let Some((slot, len)) = cache.find_best_slot(key, target)
                && best.as_ref().is_none_or(|(_, best_len, _)| len > *best_len)
            {
                best = Some((slot, len, i));
            }
        }

        best.map(|(slot, _, tier)| (slot, tier))
    }

    /// Resolves a type along the current `path`, lazily constructing it from
    /// its dependencies if needed and returning a handle plus its placement
    /// tier.
    ///
    /// # Panics
    ///
    /// Panics if a slot's stored type does not match the resolved type,
    /// which would indicate a bug in slot bookkeeping (slots are keyed by
    /// `TypeId` of the value they hold).
    pub fn resolve<O>(&mut self, path: &PathStack<'_>) -> ResolveOutput<O>
    where
        O: ResolveFrom<T>,
    {
        let target = TypeId::of::<O>();
        // Search key for `O` is the current path with `O` appended.
        let mut key = path.to_vec();
        key.push(target);

        // 1. Best slot across self + ancestors.
        let target_slot = self.find_best_slot(&key, target);

        // 2. If a matching slot is filled, return the value.
        if let Some((slot, tier)) = &target_slot
            && let Some(any) = slot.get()
        {
            let value = Arc::clone(any)
                .downcast::<O>()
                .expect("slot type must match resolved type by construction");
            return ResolveOutput::new(value, *tier);
        }

        // 3. Construct `O` after ensuring its dependencies along the extended path.
        let child_path = path.push(target);
        let deps_tier = <<O as ResolveFrom<T>>::Inputs as ResolutionDeps<T>>::ensure_all(self, &child_path);
        let inputs = <<O as ResolveFrom<T>>::Inputs as ResolutionDeps<T>>::collect(self, &child_path);
        let new_value = O::new(inputs);

        // 4. Place the value.
        if let Some((slot, tier)) = target_slot {
            // Pre-existing empty slot from a `provide()` chain: fill it. Tier
            // is set by the resolver that owns the slot.
            let new_arc: Arc<dyn std::any::Any + Send + Sync> = Arc::new(new_value);
            let _ = slot.set(new_arc);
            let stored = Arc::clone(slot.get().expect("just set or already set"));
            let typed = stored
                .downcast::<O>()
                .expect("slot type must match resolved type by construction");
            ResolveOutput::new(typed, tier)
        } else {
            // Classical placement: at the single-element path `[O]` on the
            // resolver at `deps_tier` (max of deps' tiers).
            let placement = Arc::clone(self.cache_at(deps_tier));
            let single_key = vec![target];
            let stored = placement.get_or_insert(single_key, new_value);
            ResolveOutput::new(stored, deps_tier)
        }
    }

    /// Internal accessor used by [`ResolutionDeps::collect`] to retrieve an
    /// already-resolved dependency without triggering further resolution.
    ///
    /// Performs the same longest-suffix lookup as [`resolve`](Self::resolve).
    pub(crate) fn lookup_for_collect<O: Send + Sync + 'static>(&self, path: &PathStack<'_>) -> Option<Arc<O>> {
        let target = TypeId::of::<O>();
        let mut key = path.to_vec();
        key.push(target);
        let (slot, _) = self.find_best_slot(&key, target)?;
        let any = Arc::clone(slot.get()?);
        any.downcast::<O>().ok()
    }
}
