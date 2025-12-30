// Copyright (c) Microsoft Corporation.

use std::fmt::Debug;
use std::sync::Arc;

use crate::Service;
use crate::service::DynService;

/// Extension trait that adds type erasure capabilities to any [`Service`].
///
/// This trait provides a convenient way to convert any service into a [`DynamicService`],
/// which erases the underlying concrete type. This is particularly useful when
/// working with complex service compositions or when you need to store services of
/// different types.
pub trait DynamicServiceExt<In, Out>: Sized {
    /// Converts the service into a type-erased version that hides the concrete type.
    ///
    /// This method consumes the service and returns a [`DynamicService`] that wraps
    /// the original service. The dynamic service implements the same [`Service`] trait,
    /// but with the concrete type erased, allowing it to be used in contexts where the
    /// underlying implementation type needs to be hidden.
    ///
    /// # Type Requirements
    ///
    /// - `In` must be `Send + 'static` to support async execution across threads
    /// - `Out` must be `Send + 'static` for the same reason
    /// - The service itself must be `'static`
    ///
    /// # Performance
    ///
    /// Type erasure introduces some overhead compared to concrete types.
    /// For most applications, this overhead is negligible.
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

/// A type-erased wrapper for [`Service`] that hides the concrete type.
///
/// `DynamicService` erases the underlying service implementation type, allowing you to work
/// with services of different concrete types through a uniform interface. This is particularly
/// useful when dealing with complex service compositions involving multiple layers of middleware,
/// where the resulting type can become unwieldy or when you need to store services of different
/// types in collections.
///
/// # Type Erasure
///
/// When you compose services with multiple layers of middleware, the resulting type can become
/// deeply nested and complex. For example:
///
/// ```text
/// Logging<Timeout<Retry<RateLimit<DatabaseService>>>>
/// ```
///
/// `DynamicService` allows you to erase this complexity:
///
/// ```rust
/// use layered::{DynamicService, DynamicServiceExt, Service};
///
/// // Instead of working with the complex concrete type, use DynamicService
/// let service: DynamicService<String, String> = build_complex_service().into_dynamic();
///
/// # fn build_complex_service() -> impl Service<String, Out = String> {
/// #     layered::Execute::new(|val| async move { val })
/// # }
/// ```
///
/// # When to Use
///
/// - **Complex service stacks**: When your service composition involves many layers
/// - **Service collections**: When you need to store different service types in vectors or maps
/// - **API boundaries**: When you want to hide implementation details from consumers
/// - **Conditional service selection**: When you need to choose between different service
///   implementations at runtime
///
/// # Performance
///
/// Type erasure introduces some overhead compared to concrete types. For most
/// applications, this overhead is negligible compared to the actual work performed by
/// the service (database queries, network calls, etc.).
///
/// # Examples
///
/// ```rust
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
pub struct DynamicService<In, Out>(Arc<DynService<'static, In, Out>>);

impl<In: Send + 'static, Out: Send + 'static> DynamicService<In, Out> {
    pub(crate) fn new<T>(strategy: T) -> Self
    where
        T: Service<In, Out = Out> + Send + Sync + 'static,
    {
        Self(DynService::new_arc(strategy))
    }
}

impl<In: Send, Out: Send> Service<In> for DynamicService<In, Out> {
    type Out = Out;

    async fn execute(&self, input: In) -> Self::Out {
        self.0.execute(input).await
    }
}

impl<In, Out> Debug for DynamicService<In, Out> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynamicService").finish()
    }
}

impl<In, Out> Clone for DynamicService<In, Out> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

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
}
