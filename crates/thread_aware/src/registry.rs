// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Building blocks for runtimes and thread-aware hosts.

use std::collections::HashMap;
use std::num::NonZero;
use std::sync::Mutex;
use std::thread::ThreadId;

use crate::affinity::{MemoryAffinity, PinnedAffinity};
use many_cpus::SystemHardware;

const POISONED_LOCK_MSG: &str = "poisoned lock means type invariants may not hold - not safe to continue execution";

/// The number of processors to use for the registry.
///
/// This can be set to `Auto` to use the default number of processors,
/// or `Manual` to specify a specific number of processors.
/// The `All` variant is used to specify that all processors should be used.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ProcessorCount {
    /// Use the default number of processors. Right now this is equivalent to using
    /// all processors, but this default may change in the future.
    #[default]
    Auto,
    /// Use a specific number of processors.
    Manual(NonZero<usize>),
    /// Use all processors.
    All,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct NumaNode(usize);

impl NumaNode {
    const fn invalid() -> Self {
        Self(usize::MAX)
    }

    const fn is_invalid(self) -> bool {
        self.0 == usize::MAX
    }
}

/// A registry for managing pinning threads to specific processors.
#[derive(Debug)]
pub struct ThreadRegistry {
    threads: Mutex<HashMap<ThreadId, PinnedAffinity>>,
    processors: Vec<Processor>,
    numa_nodes: Vec<NumaNode>,
}

impl ThreadRegistry {
    /// Create a new `ThreadRegistry` using the current system hardware.
    ///
    /// # Panics
    ///
    /// This will panic if there are not enough processors available when using `Manual` or if no processors are available when using `Auto` or `All`.
    /// If there are more than `u16::MAX` processors or memory regions.
    #[must_use]
    pub fn new(count: &ProcessorCount) -> Self {
        Self::with_hardware(count, SystemHardware::current())
    }

    /// Create a new `ThreadRegistry` with the specified hardware instance.
    #[must_use]
    pub(crate) fn with_hardware(count: &ProcessorCount, hardware: &SystemHardware) -> Self {
        let builder = hardware.processors().to_builder();

        let processors = match count {
            ProcessorCount::Auto | ProcessorCount::All => builder.take_all(),
            ProcessorCount::Manual(count) => builder.take(*count),
        }
        .expect("Not enough processors available");

        let mut numa_nodes = Vec::new();
        let mut dense_index = 0;
        for processor in &processors {
            let index = processor.memory_region_id() as usize;

            // Resize if needed
            if index >= numa_nodes.len() {
                numa_nodes.resize(index + 1, NumaNode::invalid());
            }

            if numa_nodes[index].is_invalid() {
                numa_nodes[index] = NumaNode(dense_index);
                dense_index += 1;
            }
        }

        assert!(processors.len() < u16::MAX as usize, "Too many processors");
        assert!(numa_nodes.len() < u16::MAX as usize, "Too many memory regions");

        Self {
            processors: Processor::unpack(&processors),
            numa_nodes,
            threads: Mutex::new(HashMap::new()),
        }
    }

    /// Get an iterator over all available memory affinities.
    #[expect(clippy::cast_possible_truncation, reason = "Checked in new()")]
    pub fn affinities(&self) -> impl Iterator<Item = PinnedAffinity> {
        self.processors.iter().enumerate().map(|(core_index, processor)| {
            let dense_numa_index = self.numa_nodes[processor.memory_region_id()];

            PinnedAffinity::new(
                core_index as _,
                dense_numa_index.0 as _,
                self.processors.len() as _,
                self.numa_nodes.len() as _,
            )
        })
    }

    /// The number of total available memory affinities.
    #[must_use]
    pub fn num_affinities(&self) -> usize {
        self.processors.len()
    }

    /// Get the memory affinity of the current thread, if it has been pinned.
    ///
    /// # Panics
    ///
    /// This will panic if the internal lock is poisoned.
    #[must_use]
    pub fn current_affinity(&self) -> MemoryAffinity {
        self.threads
            .lock()
            .expect(POISONED_LOCK_MSG)
            .get(&std::thread::current().id())
            .copied()
            .map_or(MemoryAffinity::Unknown, MemoryAffinity::Pinned)
    }

    /// Pins the current thread to the specified memory affinity.
    ///
    /// # Panics
    ///
    /// This will panic if affinity contains incorrect processor index
    pub fn pin_to(&self, affinity: PinnedAffinity) {
        let core_index = affinity.processor_index();
        let processor = &self.processors[core_index];
        processor.pin_current_thread_to();
        self.threads
            .lock()
            .expect(POISONED_LOCK_MSG)
            .insert(std::thread::current().id(), affinity);
    }
}

impl Default for ThreadRegistry {
    fn default() -> Self {
        Self::new(&ProcessorCount::Auto)
    }
}

/// A wrapper around `many_cpus::ProcessorSet` that contains only a single processor
#[derive(Debug)]
struct Processor {
    inner: many_cpus::ProcessorSet,
}

impl Processor {
    /// Unpack a `ProcessorSet` containing multiples processors into a set of `Processor` each
    /// representing a single unique processor.
    fn unpack(processor_set: &many_cpus::ProcessorSet) -> Vec<Self> {
        let mut this = processor_set
            .decompose()
            .into_iter()
            .map(|set| Self { inner: set })
            .collect::<Vec<_>>();
        this.sort_by_key(|p| p.as_processor().id());
        this
    }

    fn memory_region_id(&self) -> usize {
        self.as_processor().memory_region_id() as usize
    }

    fn pin_current_thread_to(&self) {
        self.inner.pin_current_thread_to();
    }

    fn as_processor(&self) -> &many_cpus::Processor {
        self.inner
            .iter()
            .next()
            .expect("ProcessorSet should contain one and only one processor")
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use crate::affinity::{memory_affinities, pinned_affinities};
    use crate::registry::{NumaNode, ProcessorCount, ThreadRegistry};
    use std::num::NonZero;

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_registry() {
        let registry = ThreadRegistry::default();
        for i in registry.affinities() {
            assert!(i.processor_index() < i.processor_count());
            assert!(i.memory_region_index() < i.memory_region_count());
        }

        assert!(registry.num_affinities() > 0);
        assert!(registry.current_affinity().is_unknown());

        let first = registry.affinities().next().unwrap();
        registry.pin_to(first);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_registry_manual() {
        let registry = ThreadRegistry::new(&ProcessorCount::Manual(NonZero::new(1).unwrap()));
        assert_eq!(registry.num_affinities(), 1);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_num_affinities_matches_iterator_count() {
        // This test ensures num_affinities() returns the actual count, not a constant like 1
        let registry = ThreadRegistry::default();
        let iterator_count = registry.affinities().count();
        assert_eq!(registry.num_affinities(), iterator_count);

        // Also test with manual processor count > 1 if available
        if iterator_count > 1 {
            let count = NonZero::new(2.min(iterator_count)).unwrap();
            let registry_manual = ThreadRegistry::new(&ProcessorCount::Manual(count));
            assert_eq!(registry_manual.num_affinities(), count.get());
            assert_eq!(registry_manual.num_affinities(), registry_manual.affinities().count());
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_pin_to_actually_pins() {
        // This test ensures pin_to() actually updates the thread's affinity
        let registry = ThreadRegistry::default();

        // Before pinning, affinity should be unknown
        assert!(registry.current_affinity().is_unknown());

        // Pin to the first affinity
        let first = registry.affinities().next().unwrap();
        registry.pin_to(first);

        // After pinning, affinity should be pinned and match what we set
        let current = registry.current_affinity();
        assert!(!current.is_unknown());
        assert_eq!(current, crate::affinity::MemoryAffinity::Pinned(first));
    }

    #[test]
    fn test_numa_node() {
        let invalid = NumaNode::invalid();
        assert!(invalid.is_invalid());
        assert!(!NumaNode(0).is_invalid());
        assert!(!NumaNode(123).is_invalid());
    }

    #[test]
    fn test_crate_fake_affinities() {
        let affinities = pinned_affinities(&[2, 3]);
        assert_eq!(affinities.len(), 5);
        for (i, affinity) in affinities.iter().enumerate() {
            assert_eq!(affinity.processor_index(), i);
            assert_eq!(affinity.processor_count(), 5);
            assert_eq!(affinity.memory_region_index(), usize::from(i >= 2));
            assert_eq!(affinity.memory_region_count(), 2);
        }
    }

    #[test]
    fn test_crate_fake_memory_affinities() {
        let affinities = memory_affinities(&[2, 3]);
        assert_eq!(affinities.len(), 5);
    }
}

/// Tests using fake hardware from `many_cpus::fake` for deterministic coverage
/// of multi-NUMA topologies and specific processor counts.
#[cfg(test)]
mod test_fake_hardware {
    use std::collections::HashSet;

    use super::*;
    use many_cpus::fake::HardwareBuilder;

    macro_rules! nz {
        ($e:expr) => {
            NonZero::new($e).unwrap()
        };
    }

    /// Helper to create a `ThreadRegistry` from fake hardware with the given counts.
    fn registry_from_fake(policy: &ProcessorCount, processors: usize, numa_nodes: usize) -> ThreadRegistry {
        let hw = SystemHardware::fake(HardwareBuilder::from_counts(nz!(processors), nz!(numa_nodes)));
        ThreadRegistry::with_hardware(policy, &hw)
    }

    #[test]
    #[expect(clippy::needless_collect, reason = "collect needed for pattern matching on array")]
    fn single_processor_single_numa() {
        let registry = registry_from_fake(&ProcessorCount::Auto, 1, 1);

        assert_eq!(registry.num_affinities(), 1);
        let [aff] = registry.affinities().collect::<Vec<_>>()[..] else {
            panic!("Expected exactly one affinity")
        };
        assert_eq!(aff.processor_index(), 0);
        assert_eq!(aff.memory_region_index(), 0);
        assert_eq!(aff.processor_count(), 1);
        assert_eq!(aff.memory_region_count(), 1);
    }

    #[test]
    fn auto_and_all_with_single_numa_node() {
        for policy in [ProcessorCount::Auto, ProcessorCount::All] {
            let registry = registry_from_fake(&policy, 4, 1);

            assert_eq!(registry.num_affinities(), 4);
            for aff in registry.affinities() {
                assert_eq!(aff.memory_region_index(), 0);
                assert_eq!(aff.memory_region_count(), 1);
                assert_eq!(aff.processor_count(), 4);
            }
        }
    }

    #[test]
    fn manual_subset_of_processors() {
        let registry = registry_from_fake(&ProcessorCount::Manual(nz!(3)), 8, 2);

        assert_eq!(registry.num_affinities(), 3);
        assert_eq!(registry.affinities().count(), 3);

        let registry = registry_from_fake(&ProcessorCount::Manual(nz!(1)), 8, 2);
        assert_eq!(registry.num_affinities(), 1);

        let registry = registry_from_fake(&ProcessorCount::Manual(nz!(8)), 8, 2);
        assert_eq!(registry.num_affinities(), 8);
    }

    #[test]
    #[should_panic(expected = "Not enough processors available")]
    fn manual_exceeds_available_panics() {
        let _registry = registry_from_fake(&ProcessorCount::Manual(nz!(5)), 2, 1);
    }

    #[test]
    fn multi_numa_dense_indexing() {
        for (num_procs, num_numa) in [(4, 2), (6, 3)] {
            let registry = registry_from_fake(&ProcessorCount::Auto, num_procs, num_numa);

            assert_eq!(registry.num_affinities(), num_procs);

            let affinities: Vec<_> = registry.affinities().collect();
            assert_eq!(affinities.len(), num_procs);

            let regions: HashSet<_> = affinities.iter().map(|a| a.memory_region_index()).collect();
            assert_eq!(regions.len(), num_numa);

            for aff in &affinities {
                assert_eq!(aff.processor_count(), num_procs);
                assert_eq!(aff.memory_region_count(), num_numa);
            }
        }
    }

    #[test]
    fn pin_to_updates_on_repin() {
        let registry = registry_from_fake(&ProcessorCount::Auto, 4, 2);

        let first = registry.affinities().next().unwrap();
        registry.pin_to(first);
        assert_eq!(registry.current_affinity(), crate::affinity::MemoryAffinity::Pinned(first));

        // Re-pin to a different affinity.
        let third = registry.affinities().nth(2).unwrap();
        registry.pin_to(third);
        assert_eq!(registry.current_affinity(), crate::affinity::MemoryAffinity::Pinned(third));
    }
}
