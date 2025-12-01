// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use crate::{MemoryAffinity, PinnedAffinity, ThreadAware, ThreadRegistry};

/// A validator that checks if the current thread is running on its home memory affinity.
///
/// This can be used to ensure that a thread has not been moved to another memory affinity
/// without being correctly relocated.
#[derive(Debug, Clone)]
pub struct ThreadAwareValidator {
    home: MemoryAffinity,
    registry: Arc<ThreadRegistry>,
}

impl ThreadAwareValidator {
    /// Creates a new validator for the given home memory affinity and thread registry.
    pub fn with_affinity(home: impl Into<MemoryAffinity>, registry: Arc<ThreadRegistry>) -> Self {
        Self {
            home: home.into(),
            registry,
        }
    }

    /// Checks if the current thread is running on its home memory affinity.
    ///
    /// If the validator has been send to another thread without being correctly relocated,
    /// this will return false.
    ///
    /// If the current thread has not been pinned, this will also return false.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let current_affinity = self.registry.current_affinity();

        current_affinity == self.home
    }
}

impl ThreadAware for ThreadAwareValidator {
    fn relocated(mut self, _source: MemoryAffinity, destination: PinnedAffinity) -> Self {
        self.home = destination.into();
        self
    }
}

// Skip tests with miri
#[cfg(all(test, not(miri)))]
mod tests {
    use std::num::NonZero;

    use super::*;
    use crate::ProcessorCount;

    #[mutants::skip]
    fn make_registry() -> ThreadRegistry {
        ThreadRegistry::new(&ProcessorCount::Manual(NonZero::new(2).expect("NonZero")))
    }

    #[test]
    fn validator_not_pinned_returns_false() {
        let registry = make_registry();

        let affinities: Vec<_> = registry.affinities().collect();

        // Use first available affinity as "home" but do not pin current thread.
        let home = affinities.first().copied().expect("Need at least one affinity");

        let validator = ThreadAwareValidator::with_affinity(home, Arc::new(registry));

        assert!(!validator.is_valid(), "Validator should be invalid when thread not pinned");
    }

    #[test]
    fn validator_valid_after_pin() {
        let registry = make_registry();
        let affinities: Vec<_> = registry.affinities().collect();

        let home = *affinities.first().expect("Need first affinity");

        registry.pin_to(home); // Pin thread first

        let validator = ThreadAwareValidator::with_affinity(home, Arc::new(registry));

        assert!(
            validator.is_valid(),
            "Validator should be valid when pinned to home memory affinity"
        );
    }

    #[test]
    fn validator_invalid_when_pinned_to_other_affinity() {
        let registry = make_registry();
        let affinities: Vec<_> = registry.affinities().collect();

        let home = affinities[0];
        let other = affinities[1];

        registry.pin_to(other);

        let validator = ThreadAwareValidator::with_affinity(home, Arc::new(registry));

        assert!(
            !validator.is_valid(),
            "Validator should be invalid when pinned to different memory affinity"
        );
    }

    #[test]
    fn validator_relocated_updates_home() {
        let registry = make_registry();
        let affinities: Vec<_> = registry.affinities().collect();

        let affinity_a = affinities[0];
        let affinity_b = affinities[1];

        let validator = ThreadAwareValidator::with_affinity(affinity_a, Arc::new(registry));

        assert!(!validator.is_valid()); // not pinned yet

        let registry_ref = Arc::clone(&validator.registry);
        let relocated = validator.relocated(affinity_a.into(), affinity_b);
        registry_ref.pin_to(affinity_b);
        assert!(
            relocated.is_valid(),
            "Relocated validator should be valid after pinning to new home"
        );
    }
}
