use std::marker::PhantomData;

use type_map::concurrent::TypeMap;

use crate::resolve_deps::ResolutionDeps;
use crate::resolve_from::ResolveFrom;

pub struct Resolver<T> {
    types: TypeMap,
    base: PhantomData<T>,
}

impl<T: Send + Sync + 'static> Resolver<T> {
    pub fn new(t: T) -> Self {
        let mut type_map = TypeMap::new();
        type_map.insert(t);
        Resolver {
            types: type_map,
            base: PhantomData,
        }
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
        // Weird way of doing this as I couldn't quickly figure out a good way to make lifetimes happy
        if !self.types.contains::<O>() {
            let inputs = <<O as ResolveFrom<T>>::Inputs as ResolutionDeps<T>>::get(self);
            let result = O::new_resolved_from(inputs);
            return self.types.entry().or_insert(result);
        }

        self.types
            .get::<O>()
            .expect("We checked that the map cointains this type, we still hold a mutable reference")
    }
}
