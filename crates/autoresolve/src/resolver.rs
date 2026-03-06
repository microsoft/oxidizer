use std::marker::PhantomData;
use std::sync::Arc;

use type_map::concurrent::TypeMap;

use crate::base_type::BaseType;
use crate::resolve_deps::ResolutionDeps;
use crate::resolve_from::ResolveFrom;
use crate::resolver_store::ResolverStore;
use crate::scoped_resolver::SharedResolver;
use crate::shared_type_map::SharedTypeMap;

pub struct Resolver<T> {
    types: TypeMap,
    base: PhantomData<T>,
}

impl<T: Send + Sync + 'static> ResolverStore<T> for Resolver<T> {
    fn resolve<O: ResolveFrom<T>>(&mut self) -> &O {
        if !self.types.contains::<O>() {
            let inputs = <<O as ResolveFrom<T>>::Inputs as ResolutionDeps<T>>::get(self);
            let result = O::new(inputs);
            return self.types.entry().or_insert(result);
        }

        self.types.get::<O>().expect("guarded by contains check above")
    }

    fn lookup<O: Send + Sync + 'static>(&self) -> Option<&O> {
        self.types.get::<O>()
    }

    fn store_value<O: Send + Sync + 'static>(&mut self, value: O) {
        self.types.insert(value);
    }
}

impl<T: Send + Sync + 'static> Resolver<T> {
    /// Creates a resolver from a base type that implements [`BaseType`].
    ///
    /// The base struct's fields are automatically inserted as root types. Use the
    /// `#[base]` proc macro to generate the [`BaseType`] implementation.
    pub fn new(base: T) -> Self
    where
        T: BaseType,
    {
        base.into_resolver()
    }

    pub fn new_empty() -> Self {
        Resolver {
            types: TypeMap::new(),
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
        self.types.get::<O>()
    }

    pub fn get<O>(&mut self) -> &O
    where
        O: ResolveFrom<T>,
    {
        ResolverStore::resolve(self)
    }

    /// Converts this resolver into a shared parent that can spawn scoped children.
    ///
    /// The shared resolver uses interior mutability, allowing scoped children to
    /// read types that were pre-resolved in the parent.
    pub fn into_shared(self) -> SharedResolver<T> {
        SharedResolver {
            types: Arc::new(SharedTypeMap::from_type_map(self.types)),
            ancestors: Vec::new(),
            base: PhantomData,
        }
    }
}
