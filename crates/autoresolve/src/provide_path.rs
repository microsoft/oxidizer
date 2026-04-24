//! Type-level tags describing the scope of a `provide()` registration.
//!
//! These tags travel as a generic parameter on the override builder so that
//! the compiler can validate every `when_injected_in::<T>()` link against the
//! [`DependencyOf`](crate::DependencyOf) marker trait emitted by
//! `#[resolvable]`.

use core::marker::PhantomData;

/// Tag for a `provide()` that has not been narrowed by any
/// `when_injected_in::<T>()` call.
///
/// An unscoped provide applies anywhere the provided type is needed and is
/// equivalent to a plain insertion into the resolver.
#[derive(Debug)]
pub struct Unscoped;

/// Tag for a `provide()` that has been extended by at least one
/// `when_injected_in::<T>()` call.
///
/// `I` is the most recently added link (the consumer named in the most recent
/// `when_injected_in`) and `Rest` is the previously accumulated tail that
/// extends toward the originally provided value.
#[derive(Debug)]
pub struct Scoped<I, Rest>(PhantomData<fn() -> (I, Rest)>);

impl<I, Rest> Scoped<I, Rest> {
    /// Constructs the tag value. Used internally by the builder; the value
    /// itself carries no data.
    #[must_use]
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<I, Rest> Default for Scoped<I, Rest> {
    fn default() -> Self {
        Self::new()
    }
}
