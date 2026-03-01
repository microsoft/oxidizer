// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;

use super::FallbackActionArgs;

crate::utils::define_fn_wrapper!(ShouldFallback<Out>(Fn(&Out) -> bool));

crate::utils::define_fn_wrapper!(SyncFallbackFn<Out>(Fn(Out, FallbackActionArgs) -> Out));
crate::utils::define_fn_wrapper!(AsyncFallbackFn<Out>(Fn(Out, FallbackActionArgs) -> Pin<Box<dyn Future<Output = Out> + Send>>));

/// Wraps either a sync or async user-supplied fallback function.
///
/// A synchronous fallback is invoked directly without boxing a future, while an
/// async fallback produces a boxed future that is `.await`ed.
#[derive(Debug)]
pub(crate) enum FallbackAction<Out> {
    Sync(SyncFallbackFn<Out>),
    Async(AsyncFallbackFn<Out>),
}

impl<Out: Send + 'static> FallbackAction<Out> {
    /// Create from a synchronous closure.
    pub(crate) fn new_sync(f: impl Fn(Out, FallbackActionArgs) -> Out + Send + Sync + 'static) -> Self {
        Self::Sync(SyncFallbackFn::new(f))
    }

    /// Create from an asynchronous closure.
    pub(crate) fn new_async<F, Fut>(f: F) -> Self
    where
        F: Fn(Out, FallbackActionArgs) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Out> + Send + 'static,
    {
        Self::Async(AsyncFallbackFn::new(move |out, args| Box::pin(f(out, args))))
    }

    /// Invoke the fallback action.
    pub(crate) async fn call(&self, out: Out, args: FallbackActionArgs) -> Out {
        match self {
            Self::Sync(f) => f.call(out, args),
            Self::Async(f) => f.call(out, args).await,
        }
    }
}

impl<Out> Clone for FallbackAction<Out> {
    fn clone(&self) -> Self {
        match self {
            Self::Sync(f) => Self::Sync(f.clone()),
            Self::Async(f) => Self::Async(f.clone()),
        }
    }
}
