// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{TypeId, type_name};
use std::fmt;

/// Runtime facilities supplied when an I/O provider is created.
///
/// This describes the client capability types offered by one proposed completion configuration.
/// It contains no native handles and may cross threads. A provider can select one supported
/// strategy here and declare only that strategy's [`CompletionRequirements`](crate::CompletionRequirements).
///
/// The runtime must supply the advertised clients on the final owning threads and validate the
/// selected requirements before creation. Advertised compatibility does not make allocation,
/// permission checks, or native registration infallible.
#[derive(Clone)]
pub struct ProviderContext {
    completion_services: Vec<TypeId>,
}

impl ProviderContext {
    /// Creates a provider context.
    ///
    /// This constructor is intended for runtime implementations and provider tests.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            completion_services: Vec::new(),
        }
    }

    /// Advertises a client capability of type `T` in the proposed configuration.
    ///
    /// Advertising the same type more than once is idempotent.
    #[must_use]
    pub fn with_completion_service<T: 'static>(mut self) -> Self {
        if !self.offers::<T>() {
            self.completion_services.push(TypeId::of::<T>());
        }
        self
    }

    /// Returns whether the proposed completion configuration offers a client of type `T`.
    #[must_use]
    pub fn offers<T: 'static>(&self) -> bool {
        self.completion_services.contains(&TypeId::of::<T>())
    }
}

impl Default for ProviderContext {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ProviderContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("completion_service_count", &self.completion_services.len())
            .finish_non_exhaustive()
    }
}
