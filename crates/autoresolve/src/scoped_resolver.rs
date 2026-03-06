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
/// [`into_shared()`](crate::Resolver::into_shared), or from a [`ScopedResolver`] via
/// [`into_shared()`](ScopedResolver::into_shared) to enable nested scoping.
///
/// The underlying type map uses interior mutability, allowing scoped children to read
/// types that were pre-resolved in the parent.
pub struct SharedResolver<T> {
    pub(crate) types: Arc<SharedTypeMap>,
    pub(crate) ancestors: Vec<Arc<SharedTypeMap>>,
    pub(crate) base: PhantomData<T>,
}

impl<T: Send + Sync + 'static> SharedResolver<T> {
    /// Creates a new scoped resolver that inherits types from this shared parent.
    ///
    /// Types pre-resolved in the parent (and its ancestors) are visible to the child.
    /// New types resolved by the child are stored locally and dropped when the scoped
    /// resolver is dropped.
    pub fn scoped(&self) -> ScopedResolver<T> {
        let mut ancestors = Vec::with_capacity(1 + self.ancestors.len());
        ancestors.push(Arc::clone(&self.types));
        ancestors.extend(self.ancestors.iter().map(Arc::clone));

        ScopedResolver {
            ancestors,
            types: TypeMap::new(),
            base: PhantomData,
        }
    }
}

/// A child resolver that reads from a shared parent and resolves new types locally.
///
/// Created from a [`SharedResolver`] via [`scoped()`](SharedResolver::scoped). Types
/// pre-resolved in the parent (and its ancestors) are shared across all scoped children.
/// New types are resolved into the local scope and dropped when this resolver is dropped.
pub struct ScopedResolver<T> {
    ancestors: Vec<Arc<SharedTypeMap>>,
    types: TypeMap,
    base: PhantomData<T>,
}

impl<T: Send + Sync + 'static> ScopedResolver<T> {
    fn lookup_in_ancestors<O: Send + Sync + 'static>(&self) -> Option<&O> {
        self.ancestors.iter().find_map(|a| a.try_get::<O>())
    }
}

impl<T: Send + Sync + 'static> ResolverStore<T> for ScopedResolver<T> {
    fn resolve<O: ResolveFrom<T>>(&mut self) -> &O {
        // Resolve into child if not present in child or any ancestor.
        // The contains/find_map checks return bool/Option, avoiding borrow conflicts
        // between the immutable ancestor lookup and the mutable resolution path.
        if !self.types.contains::<O>() && self.lookup_in_ancestors::<O>().is_none() {
            let inputs = <<O as ResolveFrom<T>>::Inputs as ResolutionDeps<T>>::get(self);
            let result = O::new(inputs);
            self.types.entry().or_insert(result);
        }

        self.types
            .get::<O>()
            .or_else(|| self.lookup_in_ancestors::<O>())
            .expect("type was just resolved or was already present in child/ancestors")
    }

    fn lookup<O: Send + Sync + 'static>(&self) -> Option<&O> {
        self.types
            .get::<O>()
            .or_else(|| self.lookup_in_ancestors::<O>())
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

    /// Converts this scoped resolver into a shared parent for nested scoping.
    ///
    /// Types resolved locally in this scope become the new shared layer. Ancestor
    /// types remain accessible to grandchildren through the ancestor chain.
    pub fn into_shared(self) -> SharedResolver<T> {
        SharedResolver {
            types: Arc::new(SharedTypeMap::from_type_map(self.types)),
            ancestors: self.ancestors,
            base: PhantomData,
        }
    }
}
