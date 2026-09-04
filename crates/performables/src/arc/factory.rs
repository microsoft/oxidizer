// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

use thread_aware::{Thread, ThreadAware};

pub(super) struct Factory<T: ?Sized> {
    materializer: Arc<dyn Materializer<T>>,
    source: Option<Thread>,
}

impl<T> Factory<T> {
    pub(super) fn from_function(constructor: fn() -> T) -> (Arc<T>, Self)
    where
        T: 'static,
    {
        let materializer = FunctionMaterializer { constructor };
        let current = Arc::new((materializer.constructor)());
        (
            current,
            Self {
                materializer: Arc::new(materializer),
                source: None,
            },
        )
    }

    pub(super) fn from_data<D>(data: D, constructor: fn(D) -> T) -> (Arc<T>, Self)
    where
        T: 'static,
        D: ThreadAware + Clone + Sync + 'static,
    {
        let materializer = DataMaterializer { data, constructor };
        let current = Arc::new((materializer.constructor)(materializer.data.clone()));
        (
            current,
            Self {
                materializer: Arc::new(materializer),
                source: None,
            },
        )
    }

    pub(super) fn clone_current() -> Self
    where
        T: Clone + Send + 'static,
    {
        Self {
            materializer: Arc::new(CloneCurrentMaterializer(PhantomData)),
            source: None,
        }
    }
}

impl<T: ?Sized> Factory<T> {
    pub(super) fn from_boxed_function(constructor: fn() -> Box<T>) -> (Arc<T>, Self)
    where
        T: 'static,
    {
        let materializer = BoxedFunctionMaterializer { constructor };
        let current = Arc::from((materializer.constructor)());
        (
            current,
            Self {
                materializer: Arc::new(materializer),
                source: None,
            },
        )
    }

    pub(super) fn from_clone_function<V>(value: V, clone_function: fn(&V) -> Box<T>) -> (Arc<T>, Self)
    where
        T: ThreadAware + 'static,
        V: Send + Sync + 'static,
    {
        let materializer = CloneFunctionMaterializer { value, clone_function };
        let current = Arc::from((materializer.clone_function)(&materializer.value));
        (
            current,
            Self {
                materializer: Arc::new(materializer),
                source: None,
            },
        )
    }

    pub(super) fn record_source(&mut self, source: Option<&Thread>) {
        if self.source.is_none() {
            self.source = source.cloned();
        }
    }

    pub(super) fn materialize(&self, current: &T, source: Option<&Thread>, destination: &Thread) -> Arc<T> {
        let source = self.source.as_ref().or(source);
        let value = self.materializer.materialize(current, source, destination);
        Arc::from(value)
    }
}

impl<T: ?Sized> fmt::Debug for Factory<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Factory").field("source", &self.source).finish_non_exhaustive()
    }
}

trait Materializer<T: ?Sized>: Send + Sync {
    fn materialize(&self, current: &T, source: Option<&Thread>, destination: &Thread) -> Box<T>;
}

struct FunctionMaterializer<T> {
    constructor: fn() -> T,
}

impl<T: 'static> Materializer<T> for FunctionMaterializer<T> {
    fn materialize(&self, _current: &T, _source: Option<&Thread>, _destination: &Thread) -> Box<T> {
        Box::new((self.constructor)())
    }
}

struct BoxedFunctionMaterializer<T: ?Sized> {
    constructor: fn() -> Box<T>,
}

impl<T: 'static + ?Sized> Materializer<T> for BoxedFunctionMaterializer<T> {
    fn materialize(&self, _current: &T, _source: Option<&Thread>, _destination: &Thread) -> Box<T> {
        (self.constructor)()
    }
}

struct DataMaterializer<D, T> {
    data: D,
    constructor: fn(D) -> T,
}

impl<D, T> Materializer<T> for DataMaterializer<D, T>
where
    D: ThreadAware + Clone + Sync + 'static,
    T: 'static,
{
    fn materialize(&self, _current: &T, source: Option<&Thread>, destination: &Thread) -> Box<T> {
        let mut data = self.data.clone();
        data.relocate(source, destination);
        Box::new((self.constructor)(data))
    }
}

struct CloneCurrentMaterializer<T: ?Sized>(PhantomData<fn(&T)>);

impl<T> Materializer<T> for CloneCurrentMaterializer<T>
where
    T: Clone + Send + 'static,
{
    fn materialize(&self, current: &T, _source: Option<&Thread>, _destination: &Thread) -> Box<T> {
        Box::new(current.clone())
    }
}

struct CloneFunctionMaterializer<V, T: ?Sized> {
    value: V,
    clone_function: fn(&V) -> Box<T>,
}

impl<V, T> Materializer<T> for CloneFunctionMaterializer<V, T>
where
    V: Send + Sync + 'static,
    T: ThreadAware + 'static + ?Sized,
{
    fn materialize(&self, _current: &T, source: Option<&Thread>, destination: &Thread) -> Box<T> {
        let mut value = (self.clone_function)(&self.value);
        value.relocate(source, destination);
        value
    }
}
