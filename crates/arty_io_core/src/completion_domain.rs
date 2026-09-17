// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt;
use std::sync::Arc;

/// The identity of one configured native completion collection domain.
///
/// A [`CompletionWaiter`](crate::CompletionWaiter) and the client services backed by it share
/// this identity. Different live domains never compare as the same domain, even when their
/// services have identical Rust types. Clones name the original domain.
///
/// This is an identity, not a native-resource owner or a source-registration token. The
/// native adapter must retain its own handles, registrations, and callback-visible storage.
/// A domain may describe a per-worker backend or a provider-shared collection arrangement.
#[derive(Clone)]
pub struct CompletionDomain {
    identity: Arc<()>,
}

impl CompletionDomain {
    /// Creates a new completion-domain identity.
    #[must_use]
    pub fn new() -> Self {
        Self { identity: Arc::new(()) }
    }

    /// Returns whether both values name the same collection domain.
    #[must_use]
    pub fn is_same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.identity, &other.identity)
    }

    /// Tags a client service as belonging to this domain.
    ///
    /// Native adapter authors use this when exposing services backed by their waiter.
    /// The tag detects mixing services from different domains; it cannot inspect native
    /// resources to establish whether the adapter supplied the correct value.
    #[must_use]
    pub fn service<T: 'static>(&self, value: T) -> CompletionService<T> {
        CompletionService {
            domain: self.clone(),
            value,
        }
    }
}

impl Default for CompletionDomain {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for CompletionDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

/// A typed native client capability tagged with its collection domain.
///
/// Create this through [`CompletionDomain::service`], then supply it through
/// [`DriverContext::with_completion_service`](crate::DriverContext::with_completion_service).
/// Drivers retrieve the client by its value type; operation records never pass through
/// this type-erased construction boundary.
///
/// The client's own ownership rules determine its thread safety and native-resource lifetime.
/// Cloning a service preserves its domain and clones its client handle.
#[derive(Clone)]
pub struct CompletionService<T> {
    pub(crate) domain: CompletionDomain,
    pub(crate) value: T,
}

impl<T> CompletionService<T> {
    /// Returns the collection domain backing this client.
    #[must_use]
    pub const fn domain(&self) -> &CompletionDomain {
        &self.domain
    }
}

impl<T> fmt::Debug for CompletionService<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}
