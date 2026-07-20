// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Defines the boxed future type returned by a middleware
/// [`tower_service::Service`] implementation.
///
/// Every middleware exposes a tower [`Service`](tower_service::Service) whose
/// future has the same shape: a `'static`, boxed, `Send` future that yields the
/// middleware `Out`. The only thing that differs from one middleware to the next
/// is the type's name (which is part of the public API and must stay distinct).
/// This macro generates that newtype together with its [`Debug`](std::fmt::Debug)
/// and [`Future`] implementations so each `service.rs` does not have to repeat
/// the boilerplate.
///
/// All generated items are gated behind `#[cfg(any(feature = "tower-service",
/// test))]`, matching the hand-written definitions they replace.
///
/// # Syntax
///
/// ```rust,ignore
/// crate::utils::boxed_future!(
///     /// Optional doc comment for the generated type.
///     pub TimeoutFuture
/// );
/// ```
macro_rules! boxed_future {
    ($(#[$meta:meta])* $vis:vis $name:ident) => {
        $(#[$meta])*
        #[cfg(any(feature = "tower-service", test))]
        $vis struct $name<Out> {
            inner: std::pin::Pin<Box<dyn Future<Output = Out> + Send>>,
        }

        #[cfg(any(feature = "tower-service", test))]
        impl<Out> std::fmt::Debug for $name<Out> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }

        #[cfg(any(feature = "tower-service", test))]
        impl<Out> Future for $name<Out> {
            type Output = Out;

            fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
                self.inner.as_mut().poll(cx)
            }
        }
    };
}

pub(crate) use boxed_future;
