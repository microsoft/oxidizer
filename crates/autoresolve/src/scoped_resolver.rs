use std::marker::PhantomData;
use std::sync::Arc;

use type_map::concurrent::TypeMap;

use crate::resolve_deps::ResolutionDeps;
use crate::resolve_from::ResolveFrom;
use crate::resolver::Resolver;
use crate::resolver_store::ResolverStore;
use crate::shared_type_map::SharedTypeMap;

/// A child resolver that reads from a shared parent and resolves new types locally.
///
/// Created from a [`Resolver`] via [`scoped()`](Resolver::scoped). Types
/// previously resolved in the parent (and its ancestors) are shared across all
/// scoped children. New types are resolved into the local scope and dropped when
/// this resolver is dropped.
///
/// When a newly-resolved type depends only on ancestor data, it is automatically
/// promoted into the appropriate ancestor via `get_or_insert()` so sibling scoped
/// resolvers can reuse it (see `depth` field).
pub struct ScopedResolver<T> {
    ancestors: Vec<Arc<SharedTypeMap>>,
    types: TypeMap,
    /// Tracks the shallowest scope tier that contributed a dependency during
    /// the current resolution chain. `None` means no dependencies have been
    /// seen yet; `Some(0)` means local data was used; `Some(n)` for n >= 1
    /// means the shallowest dependency came from `ancestors[n - 1]`.
    depth: Option<usize>,
    base: PhantomData<T>,
}

impl<T: Send + Sync + 'static> ScopedResolver<T> {
    fn lookup_in_ancestors<O: Send + Sync + 'static>(&self) -> Option<&O> {
        self.ancestors.iter().find_map(|a| a.try_get::<O>())
    }

    /// Returns the value and its ancestor index (0-based).
    fn lookup_in_ancestors_with_index<O: Send + Sync + 'static>(&self) -> Option<(usize, &O)> {
        self.ancestors
            .iter()
            .enumerate()
            .find_map(|(i, a)| a.try_get::<O>().map(|v| (i, v)))
    }

    /// Records that a dependency was found at the given tier (0 = local,
    /// 1 = ancestors[0], 2 = ancestors[1], …). Keeps the minimum.
    fn mark(&mut self, tier: usize) {
        self.depth = Some(match self.depth {
            None => tier,
            Some(d) => d.min(tier),
        });
    }
}

impl<T: Send + Sync + 'static> ResolverStore<T> for ScopedResolver<T> {
    fn resolve<O: ResolveFrom<T>>(&mut self) -> &O {
        // Fast path: already resolved locally or in an ancestor.
        if self.types.contains::<O>() {
            self.mark(0);
            return self.types.get::<O>().expect("guarded by contains() above");
        }
        if let Some((idx, _)) = self.lookup_in_ancestors_with_index::<O>() {
            self.mark(idx + 1);
            return self
                .lookup_in_ancestors::<O>()
                .expect("guarded by lookup_in_ancestors_with_index above");
        }

        // Slow path: resolve dependencies and construct the type.
        let saved = self.depth;
        self.depth = None;

        let inputs = <<O as ResolveFrom<T>>::Inputs as ResolutionDeps<T>>::get(self);
        let result = O::new(inputs);

        let storage_depth = self.depth;
        self.depth = saved;

        match storage_depth {
            // Dependencies came from local scope — must stay local.
            Some(0) => {
                self.types.entry().or_insert(result);
                self.mark(0);
                self.types.get::<O>().expect("just inserted into local types")
            }
            // All dependencies came from ancestors[n-1] or deeper — promote.
            Some(n) => {
                let ancestor = &self.ancestors[n - 1];
                let reference = ancestor.get_or_insert(result);
                // Inline mark: direct field write enables borrow splitting
                // (self.depth is disjoint from self.ancestors).
                self.depth = Some(match self.depth {
                    None => n,
                    Some(d) => d.min(n),
                });
                reference
            }
            // No dependencies (leaf type) — promote to deepest ancestor.
            None => {
                if self.ancestors.is_empty() {
                    // No ancestors at all (shouldn't happen in practice for scoped resolvers).
                    self.types.entry().or_insert(result);
                    self.mark(0);
                    self.types.get::<O>().expect("just inserted into local types")
                } else {
                    let len = self.ancestors.len();
                    let ancestor = self.ancestors.last().expect("guarded by !is_empty() above");
                    let reference = ancestor.get_or_insert(result);
                    self.depth = Some(match self.depth {
                        None => len,
                        Some(d) => d.min(len),
                    });
                    reference
                }
            }
        }
    }

    fn lookup<O: Send + Sync + 'static>(&self) -> Option<&O> {
        self.types.get::<O>().or_else(|| self.lookup_in_ancestors::<O>())
    }

    fn store_value<O: Send + Sync + 'static>(&mut self, value: O) {
        self.types.insert(value);
    }
}

impl<T: Send + Sync + 'static> ScopedResolver<T> {
    pub(crate) fn new_with_ancestors(ancestors: Vec<Arc<SharedTypeMap>>) -> Self {
        ScopedResolver {
            ancestors,
            types: TypeMap::new(),
            depth: None,
            base: PhantomData,
        }
    }

    pub fn insert<V: Send + Sync + 'static>(&mut self, value: V) {
        self.types.insert(value);
    }

    pub fn ensure<O>(&mut self) -> &O
    where
        O: ResolveFrom<T>,
    {
        self.get::<O>()
    }

    pub fn try_get<O>(&self) -> Option<&O>
    where
        O: ResolveFrom<T>,
    {
        ResolverStore::lookup(self)
    }

    pub fn get<O>(&mut self) -> &O
    where
        O: ResolveFrom<T>,
    {
        ResolverStore::resolve(self)
    }

    /// Converts this scoped resolver into a [`Resolver`] for nested scoping.
    ///
    /// Types resolved locally in this scope become the new shared layer. Ancestor
    /// types remain accessible to grandchildren through the ancestor chain.
    pub fn into_shared(self) -> Resolver<T> {
        Resolver {
            types: Arc::new(SharedTypeMap::from_type_map(self.types)),
            ancestors: self.ancestors,
            base: PhantomData,
        }
    }
}
