use std::marker::PhantomData;
use std::sync::Arc;

use type_map::concurrent::TypeMap;

use crate::base_type::{BaseType, ScopedUnder};
use crate::resolve_deps::ResolutionDeps;
use crate::resolve_from::ResolveFrom;
use crate::resolver_store::ResolverStore;
use crate::scoped_resolver::ScopedResolver;
use crate::shared_type_map::SharedTypeMap;

/// A resolver that lazily constructs types from their dependencies.
///
/// Uses interior mutability so it can also serve as a shared parent for
/// [`ScopedResolver`] children via [`scoped()`](Resolver::scoped).
pub struct Resolver<T> {
    pub(crate) types: Arc<SharedTypeMap>,
    pub(crate) ancestors: Vec<Arc<SharedTypeMap>>,
    pub(crate) base: PhantomData<T>,
}

impl<T: Send + Sync + 'static> ResolverStore<T> for Resolver<T> {
    fn resolve<O: ResolveFrom<T>>(&mut self) -> &O {
        if self.types.contains::<O>() {
            return self.types.try_get::<O>().expect("guarded by contains() above");
        }
        if self.ancestors.iter().any(|a| a.contains::<O>()) {
            return self
                .ancestors
                .iter()
                .find_map(|a| a.try_get::<O>())
                .expect("guarded by any() check above");
        }

        let inputs = <<O as ResolveFrom<T>>::Inputs as ResolutionDeps<T>>::get(self);
        let result = O::new(inputs);
        self.types.get_or_insert(result)
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
        T: BaseType<T>,
    {
        let mut resolver = Self::new_empty();
        base.insert_into(&mut resolver);
        resolver
    }

    pub fn new_empty() -> Self {
        Resolver {
            types: Arc::new(SharedTypeMap::from_type_map(TypeMap::new())),
            ancestors: Vec::new(),
            base: PhantomData,
        }
    }

    pub fn insert<V: Send + Sync + 'static>(&mut self, value: V) {
        self.types.get_or_insert(value);
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

    /// Creates a new scoped resolver that inherits types from this resolver.
    ///
    /// The scoped base struct's fields are automatically inserted as root types.
    /// Types already resolved in this resolver (and its ancestors) are visible to
    /// the child. New types resolved by the child are stored locally and dropped
    /// when the scoped resolver is dropped.
    pub fn scoped<S>(&self, roots: S) -> ScopedResolver<S>
    where
        S: BaseType<S> + ScopedUnder<Parent = T>,
    {
        let mut ancestors = Vec::with_capacity(1 + self.ancestors.len());
        ancestors.push(Arc::clone(&self.types));
        ancestors.extend(self.ancestors.iter().map(Arc::clone));

        let mut resolver = ScopedResolver::new_with_ancestors(ancestors);
        roots.insert_into(&mut resolver);
        resolver
    }
}
