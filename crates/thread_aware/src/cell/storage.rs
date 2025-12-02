// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;

use crate::PinnedAffinity;

/// A strategy for storing data in a affinity-aware manner.
pub trait Strategy {
    /// Returns the slot index for the given affinity.
    fn index(affinity: PinnedAffinity) -> usize;

    /// Returns the total number of slots for the given affinity.
    fn count(affinity: PinnedAffinity) -> usize;
}

/// A strategy that stores data per processor.
///
/// This strategy uses the processor index and count from the `PinnedAffinity` to determine
/// where to store and retrieve data.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct PerCore;
impl Strategy for PerCore {
    fn index(affinity: PinnedAffinity) -> usize {
        affinity.processor_index()
    }

    fn count(affinity: PinnedAffinity) -> usize {
        affinity.processor_count()
    }
}

/// A strategy that stores data per memory region.
///
/// This strategy uses the memory region index and count from the `PinnedAffinity` to determine
/// where to store and retrieve data.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct PerNuma;
impl Strategy for PerNuma {
    fn index(affinity: PinnedAffinity) -> usize {
        affinity.memory_region_index()
    }

    fn count(affinity: PinnedAffinity) -> usize {
        affinity.memory_region_count()
    }
}

/// A strategy that stores data per process.
///
/// This strategy does not differentiate between affinities, storing all data in a single slot.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct PerProcess;
impl Strategy for PerProcess {
    fn index(_affinity: PinnedAffinity) -> usize {
        0
    }

    fn count(_affinity: PinnedAffinity) -> usize {
        1
    }
}

/// Type used for storing data in a affinity-aware manner.
///
/// This type is used to manage the data for each affinity, depending on the chosen strategy.
#[derive(Debug)]
pub struct Storage<T, S: Strategy> {
    data: Vec<Option<T>>,
    _marker: PhantomData<S>,
}

impl<T, S: Strategy> Storage<T, S> {
    /// Creates a new empty `Storage` instance.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            data: Vec::new(),
            _marker: PhantomData,
        }
    }

    /// Replaces the data for the given affinity with the provided value.
    ///
    /// Returns the previous value if it existed, otherwise returns `None`.
    pub fn replace(&mut self, affinity: PinnedAffinity, value: T) -> Option<T> {
        self.resize(S::count(affinity));

        self.data[S::index(affinity)].replace(value)
    }

    #[cfg_attr(test, mutants::skip)] // Mutates < to <= which does not change observable behavior.
    fn resize(&mut self, num_affinities: usize) {
        if self.data.len() < num_affinities {
            self.data.resize_with(num_affinities, || None);
        }
    }
}

impl<T, S: Strategy> Default for Storage<T, S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, S: Strategy> Storage<T, S>
where
    T: Clone,
{
    /// Clone and gets the data for the given affinity if it exists.
    /// Returns `None` if the data does not exist for that affinity.
    #[must_use]
    pub fn get_clone(&self, affinity: PinnedAffinity) -> Option<T> {
        self.data.get(S::index(affinity)).and_then(std::clone::Clone::clone)
    }
}

/// Storage type that uses the [`PerCore`] strategy.
pub type PerCoreStorage<T> = Storage<T, PerCore>;

/// Storage type that uses the [`PerNuma`] strategy.
pub type PerNumaStorage<T> = Storage<T, PerNuma>;

/// Storage type that uses the [`PerProcess`] strategy.
pub type PerAppStorage<T> = Storage<T, PerProcess>;

#[cfg(test)]
mod tests {
    use crate::{Storage, Strategy, create_manual_pinned_affinities};

    #[test]
    fn replace_returns_previous_value() {
        use super::PerCoreStorage;

        let affinities = create_manual_pinned_affinities(&[1]);
        let mut storage = PerCoreStorage::new();
        let affinity = affinities[0];

        // First replace should return None (no previous value)
        let previous = storage.replace(affinity, "First".to_string());
        assert_eq!(previous, None);

        // Second replace should return the previous value
        let previous = storage.replace(affinity, "Second".to_string());
        assert_eq!(previous, Some("First".to_string()));

        // Third replace should return the new previous value
        let previous = storage.replace(affinity, "Third".to_string());
        assert_eq!(previous, Some("Second".to_string()));
    }

    #[test]
    fn get_clone() {
        use super::PerCoreStorage;

        let affinities = create_manual_pinned_affinities(&[1]);

        let mut storage = PerCoreStorage::new();
        let affinity = affinities[0];

        assert!(storage.get_clone(affinity).is_none());

        storage.replace(affinity, "Hello".to_string());
        assert_eq!(storage.get_clone(affinity), Some("Hello".to_string()));
    }

    #[test]
    fn per_app() {
        let affinities = create_manual_pinned_affinities(&[1, 1]);

        let index = super::PerProcess::index(affinities[0]);
        let count = super::PerProcess::count(affinities[0]);
        assert_eq!(index, 0);
        assert_eq!(count, 1);
    }

    #[test]
    fn per_memory_region() {
        let affinities = create_manual_pinned_affinities(&[1, 1]);

        for affinity in affinities {
            let index = super::PerNuma::index(affinity);
            let count = super::PerNuma::count(affinity);
            assert_eq!(index, affinity.memory_region_index());
            assert_eq!(count, affinity.memory_region_count());
        }
    }

    #[test]
    fn per_processor() {
        let affinities = create_manual_pinned_affinities(&[1, 1]);

        for affinity in affinities {
            let index = super::PerCore::index(affinity);
            let count = super::PerCore::count(affinity);
            assert_eq!(index, affinity.processor_index());
            assert_eq!(count, affinity.processor_count());
        }
    }

    #[test]
    fn test_default_implementation() {
        // This test covers line 101: Self::new() in the Default trait implementation
        use super::PerCoreStorage;

        let affinities = create_manual_pinned_affinities(&[1]);

        // Create storage using Default trait - this exercises line 101
        let mut storage: PerCoreStorage<String> = Storage::default();
        let affinity = affinities[0];

        // Verify the default storage is empty (no data for any affinity)
        assert!(storage.get_clone(affinity).is_none());

        // Verify it works the same as Storage::new()
        storage.replace(affinity, "test".to_string());
        assert_eq!(storage.get_clone(affinity), Some("test".to_string()));
    }
}
