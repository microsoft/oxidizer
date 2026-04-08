use std::marker::PhantomData;
use std::sync::Arc;

use type_map::concurrent::TypeMap;

use crate::base_type::BaseType;
use crate::resolve_deps::ResolutionDeps;
use crate::resolve_from::ResolveFrom;
use crate::resolver_store::ResolverStore;
use crate::shared_type_map::SharedTypeMap;

/// A resolver that lazily constructs types from their dependencies.
///
/// Uses interior mutability (`Arc<SharedTypeMap>`) so that resolved types are
/// visible to child scopes created via [`scoped()`](Resolver::scoped).
///
/// When a resolver has ancestors (i.e. it was created by `scoped()`), newly
/// resolved types are automatically promoted into the shallowest ancestor whose
/// data was sufficient to construct them. This lets sibling scopes share
/// resolved instances without redundant construction.
pub struct Resolver<T> {
    types: Arc<SharedTypeMap>,
    ancestors: Vec<Arc<SharedTypeMap>>,
    /// Tracks the shallowest scope tier that contributed a dependency during
    /// the current resolution chain. `None` means no dependencies have been
    /// seen yet; `Some(0)` means local data was used; `Some(n)` for n >= 1
    /// means the shallowest dependency came from `ancestors[n - 1]`.
    depth: Option<usize>,
    base: PhantomData<T>,
}

impl<T: Send + Sync + 'static> Resolver<T> {
    /// Returns the value and its ancestor index (1-based tier).
    fn lookup_in_ancestors_with_tier<O: Send + Sync + 'static>(&self) -> Option<(usize, &O)> {
        self.ancestors
            .iter()
            .enumerate()
            .find_map(|(i, a)| a.try_get::<O>().map(|v| (i + 1, v)))
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

impl<T: Send + Sync + 'static> ResolverStore<T> for Resolver<T> {
    fn resolve<O: ResolveFrom<T>>(&mut self) -> &O {
        // Fast path: already resolved locally or in an ancestor.
        if self.types.contains::<O>() {
            self.mark(0);
            return self.types.try_get::<O>().expect("guarded by contains() above");
        }
        if let Some((tier, _)) = self.lookup_in_ancestors_with_tier::<O>() {
            self.mark(tier);
            return self
                .ancestors
                .iter()
                .find_map(|a| a.try_get::<O>())
                .expect("guarded by lookup_in_ancestors_with_tier above");
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
                let reference = self.types.get_or_insert(result);
                // Inline mark: direct field write enables borrow splitting
                // (self.depth is disjoint from self.types).
                self.depth = Some(match self.depth {
                    None => 0,
                    Some(d) => d.min(0),
                });
                reference
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
            // No dependencies (leaf type) — promote to deepest ancestor, or store locally.
            None => {
                if self.ancestors.is_empty() {
                    let reference = self.types.get_or_insert(result);
                    self.depth = Some(match self.depth {
                        None => 0,
                        Some(d) => d.min(0),
                    });
                    reference
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
        self.types
            .try_get::<O>()
            .or_else(|| self.ancestors.iter().find_map(|a| a.try_get::<O>()))
    }

    fn store_value<O: Send + Sync + 'static>(&mut self, value: O) {
        self.types.get_or_insert(value);
    }
}

impl<T: Send + Sync + 'static> Resolver<T> {
    /// Creates a resolver from a base type that implements [`BaseType`].
    ///
    /// The base struct's fields are automatically inserted as root types. Use the
    /// `#[base]` proc macro to generate the [`BaseType`] implementation.
    pub fn new(base: T) -> Self
    where
        T: BaseType<Parent = ()>,
    {
        let mut resolver = Self::new_empty();
        base.insert_into(&mut resolver);
        resolver
    }

    /// Creates an empty resolver with no pre-inserted types.
    pub fn new_empty() -> Self {
        Resolver {
            types: Arc::new(SharedTypeMap::from_type_map(TypeMap::new())),
            ancestors: Vec::new(),
            depth: None,
            base: PhantomData,
        }
    }

    /// Pre-inserts a value into the resolver's local store.
    pub fn insert<V: Send + Sync + 'static>(&mut self, value: V) {
        self.types.get_or_insert(value);
    }

    /// Resolves a type, lazily constructing it from its dependencies if needed.
    pub fn ensure<O>(&mut self) -> &O
    where
        O: ResolveFrom<T>,
    {
        self.get::<O>()
    }

    /// Returns a reference to an already-resolved type, or `None` if it has not been resolved.
    pub fn try_get<O>(&self) -> Option<&O>
    where
        O: ResolveFrom<T>,
    {
        ResolverStore::lookup(self)
    }

    /// Resolves a type, lazily constructing it from its dependencies if needed.
    pub fn get<O>(&mut self) -> &O
    where
        O: ResolveFrom<T>,
    {
        ResolverStore::resolve(self)
    }

    /// Creates a child resolver that inherits types from this resolver.
    ///
    /// The scoped base struct's fields are automatically inserted as root types.
    /// Types already resolved in this resolver (and its ancestors) are visible to
    /// the child. New types resolved by the child may be promoted into an ancestor
    /// if all their dependencies came from that ancestor (or deeper).
    pub fn scoped<S>(&self, roots: S) -> Resolver<S>
    where
        S: BaseType<Parent = T>,
    {
        let mut ancestors = Vec::with_capacity(1 + self.ancestors.len());
        ancestors.push(Arc::clone(&self.types));
        ancestors.extend(self.ancestors.iter().map(Arc::clone));

        let mut resolver = Resolver::<S> {
            types: Arc::new(SharedTypeMap::from_type_map(TypeMap::new())),
            ancestors,
            depth: None,
            base: PhantomData,
        };
        roots.insert_into(&mut resolver);
        resolver
    }
}
