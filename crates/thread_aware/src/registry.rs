// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Building blocks for runtimes and thread-aware hosts.

use std::collections::HashMap;
use std::num::NonZero;
use std::sync::Mutex;
use std::thread::ThreadId;

use many_cpus::{Processor, ProcessorSet};

use crate::{MemoryAffinity, PinnedAffinity};


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
    /// Create a new `ThreadRegistry`.
    ///
    /// # Parameters
    ///
    /// * `count`: The number of processors to use.
    ///
    /// # Panics
    ///
    /// This will panic if there are not enough processors available when using `Manual` or if no processors are available when using `Auto` or `All`.
    /// If there are more than `u16::MAX` processors or memory regions.
    #[must_use]
    pub fn new(count: &ProcessorCount) -> Self {
        let builder = many_cpus::ProcessorSet::builder();

        let processors: Vec<_> = match count {
            ProcessorCount::Auto | ProcessorCount::All => builder.take_all(),
            ProcessorCount::Manual(count) => builder.take(*count),
        }
            .expect("Not enough processors available")
            .processors()
            .into_iter()
            .cloned()
            .collect();

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
            processors,
            numa_nodes,
            threads: Mutex::new(HashMap::new()),
        }
    }

    /// Get an iterator over all available memory affinities.
    #[expect(clippy::cast_possible_truncation, reason = "Checked in new()")]
    pub fn affinities(&self) -> impl Iterator<Item=PinnedAffinity> {
        self.processors.iter().enumerate().map(|(core_index, processor)| {
            let dense_numa_index = self.numa_nodes[processor.memory_region_id() as usize];

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
            .expect("Failed to acquire lock")
            .get(&std::thread::current().id())
            .copied()
            .map_or(MemoryAffinity::Unknown, MemoryAffinity::Pinned)
    }

    /// Pins the current thread to the specified memory affinity.
    ///
    /// # Panics
    ///
    /// This will panic if the internal lock is poisoned.
    pub fn pin_to(&self, affinity: PinnedAffinity) {
        let core_index = affinity.processor_index();
        let processor = &self.processors[core_index];

        ProcessorSet::from_processor(processor.clone()).pin_current_thread_to();

        self.threads
            .lock()
            .expect("Failed to acquire lock")
            .insert(std::thread::current().id(), affinity);
    }
}

impl Default for ThreadRegistry {
    fn default() -> Self {
        Self::new(&ProcessorCount::Auto)
    }
}

#[cfg(test)]
mod tests {
    use crate::registry::NumaNode;
    use crate::test_util::{create_manual_memory_affinities, create_manual_pinned_affinities};


    #[test]
    fn test_numa_node() {
        let invalid = NumaNode::invalid();
        assert!(invalid.is_invalid());
        assert!(!NumaNode(0).is_invalid());
        assert!(!NumaNode(123).is_invalid());
    }


    #[test]
    fn test_crate_fake_affinities() {
        let affinities = create_manual_pinned_affinities(&[2, 3]);
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
        let affinities = create_manual_memory_affinities(&[2, 3]);
        assert_eq!(affinities.len(), 5);
        for (i, affinity) in affinities.iter().enumerate() {
            if let crate::MemoryAffinity::Pinned(affinity) = affinity {
                assert_eq!(affinity.processor_index(), i);
                assert_eq!(affinity.processor_count(), 5);
                assert_eq!(affinity.memory_region_index(), usize::from(i >= 2));
                assert_eq!(affinity.memory_region_count(), 2);
            }
        }
    }
}
