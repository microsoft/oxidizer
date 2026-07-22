// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use plurality::Pool;

use crate::Service;

/// Extension trait for converting services to [`DynamicService`].
///
/// Provides type erasure for services, useful when working with complex service
/// compositions or storing services of different types.
pub trait DynamicServiceExt<In, Out>: Sized {
    /// Converts this service into a type-erased [`DynamicService`].
    fn into_dynamic(self) -> DynamicService<In, Out>;
}

impl<In: Send + 'static, Out: Send + 'static, T> DynamicServiceExt<In, Out> for T
where
    T: Service<In, Out = Out> + 'static,
{
    fn into_dynamic(self) -> DynamicService<In, Out> {
        DynamicService::new(self)
    }
}

/// Type-erased wrapper for [`Service`] that hides the concrete type.
///
/// Use `DynamicService` when working with complex service compositions where the
/// concrete type becomes unwieldy, or when storing services of different types in
/// collections.
///
/// Type erasure adds some overhead, but it's typically negligible compared to the
/// actual service work (network calls, database queries, etc.).
///
/// # Examples
///
/// ```
/// use layered::{DynamicService, DynamicServiceExt, Execute, Service};
///
/// async fn example() {
///     // Create a concrete service that doubles an integer and uses a closure.
///     let service = Execute::new(|v: i32| async move { v * 2 });
///
///     // Create a type-erased service
///     let service: DynamicService<i32, i32> = service.into_dynamic();
///
///     // Use it like any other service
///     let result = service.execute(42).await;
///     println!("Result: {}", result);
/// }
/// ```
pub struct DynamicService<In, Out> {
    exec: Arc<dyn Fn(In) -> plurality::Box<dyn SendFuture<Out>> + Send + Sync>,
}

pub(crate) trait SendFuture<Out>: Future<Output = Out> + Send {}
impl<Out: Send + 'static, F: Future<Output = Out> + Send> SendFuture<Out> for F {}

impl<In: Send + 'static, Out: Send + 'static> DynamicService<In, Out> {
    pub(crate) fn new<T>(strategy: T) -> Self
    where
        T: Service<In, Out = Out> + Send + Sync + 'static,
    {
        // Each concrete `T` produces a single future type, so one typed pool
        // suffices. The pool is `Send + !Sync`, so a mutex guards it — expected
        // to be uncontended in the thread-isolated common case. The erased
        // handle owns its slot, so it stays valid after the lock is released.
        let pool = Mutex::new(Pool::new());
        let service = Arc::new(strategy);
        // Wrap each service execution in a future, pool it, and hand back a
        // type-erased handle so `DynamicService` need not carry the future type.
        let exec = move |input: In| {
            let cloned = Arc::clone(&service);
            let fut = async move { cloned.execute(input).await };
            let boxed = pool
                .lock()
                // Poisoned only if a prior holder panicked under this guard. No user
                // code runs here: `alloc_box` just moves the already-constructed
                // `fut` into a pool slot (the future is polled later, outside the
                // lock), so a poisoned guard indicates a fatal prior failure.
                .expect("dynamic-service pool mutex poisoned by a prior panic under this guard")
                .alloc_box(fut);
            plurality::Box::unsize::<dyn SendFuture<Out>>(boxed, plurality::coerce!(<Out> dyn SendFuture<Out>))
        };

        Self { exec: Arc::new(exec) }
    }
}

impl<In: Send, Out: Send> Service<In> for DynamicService<In, Out> {
    type Out = Out;

    fn execute(&self, input: In) -> impl Future<Output = Self::Out> + Send {
        ServiceFuture {
            handle: self.exec.as_ref()(input),
        }
    }
}

struct ServiceFuture<Out> {
    handle: plurality::Box<dyn SendFuture<Out>>,
}

impl<Out> Future for ServiceFuture<Out> {
    type Output = Out;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().handle.as_pin_mut().poll(cx)
    }
}

impl<In, Out> Debug for DynamicService<In, Out> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynamicService").finish()
    }
}

impl<In, Out> Clone for DynamicService<In, Out> {
    fn clone(&self) -> Self {
        Self {
            exec: Arc::clone(&self.exec),
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::Execute;

    #[test]
    fn assert_types() {
        assert_impl_all!(DynamicService<(), ()>: Send, Sync, Clone, Debug);

        // If non-clonable types are used, ensure the DynamicService is still cloneable
        assert_impl_all!(DynamicService<Mutex<()>, Mutex<()>>: Send, Sync, Clone, Debug);
    }

    #[test]
    fn into_dynamic() {
        let dynamic_service: DynamicService<i32, i32> = Execute::new(|v| async move { v }).into_dynamic();

        assert_eq!(block_on(dynamic_service.execute(42)), 42);
    }

    #[test]
    fn clone_and_debug() {
        let svc: DynamicService<i32, i32> = Execute::new(|v| async move { v }).into_dynamic();
        let cloned = svc.clone();
        assert_eq!(block_on(cloned.execute(1)), 1);
        assert_eq!(format!("{svc:?}"), "DynamicService");
    }
}
