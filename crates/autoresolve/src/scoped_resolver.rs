use std::marker::PhantomData;
use std::sync::Arc;

use type_map::concurrent::TypeMap;

use crate::resolve_deps::ResolutionDeps;
use crate::resolve_from::ResolveFrom;
use crate::resolver_store::ResolverStore;
use crate::shared_type_map::SharedTypeMap;

/// A shared parent resolver that can spawn scoped child resolvers.
///
/// Created from a [`Resolver`](crate::Resolver) via
/// [`into_shared()`](crate::Resolver::into_shared). The underlying type map uses interior
/// mutability, allowing scoped children to read types that were pre-resolved in the parent.
pub struct SharedResolver<T> {
    pub(crate) types: Arc<SharedTypeMap>,
    pub(crate) base: PhantomData<T>,
}

impl<T: Send + Sync + 'static> SharedResolver<T> {
    /// Creates a new scoped resolver that inherits types from this shared parent.
    ///
    /// Types pre-resolved in the parent are visible to the child. New types resolved
    /// by the child are stored locally and dropped when the scoped resolver is dropped.
    pub fn scoped(&self) -> ScopedResolver<T> {
        ScopedResolver {
            parent: Arc::clone(&self.types),
            types: TypeMap::new(),
            base: PhantomData,
        }
    }
}

/// A child resolver that reads from a shared parent and resolves new types locally.
///
/// Created from a [`SharedResolver`] via [`scoped()`](SharedResolver::scoped). Types
/// pre-resolved in the parent are shared across all scoped children. New types are
/// resolved into the local scope and dropped when this resolver is dropped.
pub struct ScopedResolver<T> {
    parent: Arc<SharedTypeMap>,
    types: TypeMap,
    base: PhantomData<T>,
}

impl<T: Send + Sync + 'static> ResolverStore<T> for ScopedResolver<T> {
    fn resolve<O: ResolveFrom<T>>(&mut self) -> &O {
        // Resolve into child if not present in child or parent.
        // The contains/is_none checks return bool, avoiding borrow conflicts
        // between the immutable parent lookup and the mutable resolution path.
        if !self.types.contains::<O>() && self.parent.try_get::<O>().is_none() {
            let inputs = <<O as ResolveFrom<T>>::Inputs as ResolutionDeps<T>>::get(self);
            let result = O::new(inputs);
            self.types.entry().or_insert(result);
        }

        self.types
            .get::<O>()
            .or_else(|| self.parent.try_get::<O>())
            .expect("type was just resolved or was already present in child/parent")
    }

    fn lookup<O: Send + Sync + 'static>(&self) -> Option<&O> {
        self.types.get::<O>().or_else(|| self.parent.try_get::<O>())
    }

    fn store_value<O: Send + Sync + 'static>(&mut self, value: O) {
        self.types.insert(value);
    }
}

impl<T: Send + Sync + 'static> ScopedResolver<T> {
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
}
