// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{TypeId, type_name};

use crate::{DriverContext, DriverError};

/// The client capabilities needed by one selected driver strategy.
///
/// All listed capabilities are required. A provider that supports alternative strategies
/// chooses one using [`ProviderContext::offers`](crate::ProviderContext::offers) before
/// returning its requirements; it does not combine alternative strategies into this set.
///
/// Requirements describe compatibility before native registration. Validation does not
/// eliminate resource or permission failures during driver creation.
#[derive(Clone, Debug, Default)]
pub struct CompletionRequirements {
    services: Vec<(TypeId, &'static str)>,
}

impl CompletionRequirements {
    /// Creates requirements for a driver that needs no native client capabilities.
    ///
    /// Such a driver still participates in the service, notification, and shutdown protocols.
    #[must_use]
    pub const fn new() -> Self {
        Self { services: Vec::new() }
    }

    /// Requires a client capability of type `T`.
    ///
    /// Requiring the same type again is idempotent.
    #[must_use]
    pub fn require<T: 'static>(mut self) -> Self {
        if !self.requires::<T>() {
            self.services.push((TypeId::of::<T>(), type_name::<T>()));
        }
        self
    }

    /// Returns whether this strategy requires a client capability of type `T`.
    #[must_use]
    pub fn requires<T: 'static>(&self) -> bool {
        self.services.iter().any(|(id, _)| *id == TypeId::of::<T>())
    }

    /// Checks that the final owning thread's context supplies every required client.
    ///
    /// The runtime performs this before calling [`DriverProvider::create`](crate::DriverProvider::create).
    ///
    /// # Errors
    ///
    /// Returns an unsupported-configuration error naming the first missing capability.
    pub fn validate(&self, context: &DriverContext) -> Result<(), DriverError> {
        for (id, name) in &self.services {
            if !context.has_completion_service(*id) {
                return Err(DriverError::missing_service(name));
            }
        }
        Ok(())
    }
}
