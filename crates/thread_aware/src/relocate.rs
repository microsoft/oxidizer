// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Test helpers for relocating thread-aware values.

use std::sync::OnceLock;
use std::thread::{self, ThreadId};

use crate::{Thread, ThreadAware, ThreadBuilder};

static SOURCE_THREAD_ID: OnceLock<ThreadId> = OnceLock::new();
static THREAD_BUILDER: OnceLock<ThreadBuilder> = OnceLock::new();

/// Relocates a value between synthetic runtime thread coordinates.
///
/// The source uses one process-wide thread ID, while the destination uses the
/// thread ID captured when the fixture is constructed. Ordinary fixtures share
/// one process-wide owner. This keeps repeated relocation tests cheap while
/// still providing distinct thread coordinates.
#[derive(Debug)]
pub struct Relocator {
    source: Thread,
    destination: Thread,
    destination_numa_node: u32,
    include_source: bool,
}

impl Relocator {
    /// Creates a relocation between two threads on the same NUMA node.
    #[must_use]
    pub fn between_threads() -> Self {
        Self::new(0, 0)
    }

    /// Creates a relocation between threads on different NUMA nodes.
    #[must_use]
    pub fn between_numa_nodes() -> Self {
        Self::new(0, 1)
    }

    /// Controls whether relocation receives the source coordinate.
    #[must_use]
    pub fn source(mut self, include_source: bool) -> Self {
        self.include_source = include_source;
        self
    }

    /// Uses an owner distinct from the source for the destination coordinate.
    #[must_use]
    pub fn different_owner(mut self) -> Self {
        self.destination = ThreadBuilder::default()
            .with_numa_node(self.destination_numa_node)
            .build(self.destination.id());
        self
    }

    /// Relocates `value` and returns the source and destination coordinates.
    #[must_use]
    pub fn relocate<T: ThreadAware + ?Sized>(&self, value: &mut T) -> (Option<Thread>, Thread) {
        let source = self.include_source.then(|| self.source.clone());
        value.relocate(source.as_ref(), &self.destination);
        (source, self.destination.clone())
    }

    fn new(source_numa_node: u32, destination_numa_node: u32) -> Self {
        let builder = THREAD_BUILDER.get_or_init(ThreadBuilder::default);
        Self {
            source: builder.clone().with_numa_node(source_numa_node).build(Self::source_thread_id()),
            destination: builder.clone().with_numa_node(destination_numa_node).build(thread::current().id()),
            destination_numa_node,
            include_source: true,
        }
    }

    fn source_thread_id() -> ThreadId {
        *SOURCE_THREAD_ID.get_or_init(|| {
            #[cfg(miri)]
            {
                return thread::current().id();
            }

            #[cfg(not(miri))]
            thread::spawn(|| thread::current().id())
                .join()
                .expect("source thread only reads its own ID and cannot panic")
        })
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Relocation {
        had_source: bool,
        crossed_numa_nodes: bool,
    }

    impl ThreadAware for Relocation {
        fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
            self.had_source = source.is_some();
            self.crossed_numa_nodes = source.is_some_and(|source| source.numa_node() != destination.numa_node());
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn between_threads_uses_distinct_threads_on_the_same_numa_node() {
        let mut relocation = Relocation {
            had_source: false,
            crossed_numa_nodes: true,
        };
        let (source, destination) = Relocator::between_threads().relocate(&mut relocation);
        let source = source.unwrap();

        assert!(relocation.had_source);
        assert!(!relocation.crossed_numa_nodes);
        assert_ne!(source.id(), destination.id());
        assert_eq!(source.owner(), destination.owner());
        assert_eq!(source.numa_node(), destination.numa_node());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn between_numa_nodes_uses_distinct_numa_nodes() {
        let mut relocation = Relocation {
            had_source: false,
            crossed_numa_nodes: false,
        };
        let (source, destination) = Relocator::between_numa_nodes().relocate(&mut relocation);
        let source = source.unwrap();

        assert!(relocation.had_source);
        assert!(relocation.crossed_numa_nodes);
        assert_ne!(source.id(), destination.id());
        assert_eq!(source.owner(), destination.owner());
        assert_ne!(source.numa_node(), destination.numa_node());
    }

    #[test]
    fn source_false_passes_no_source_coordinate() {
        let mut relocation = Relocation {
            had_source: true,
            crossed_numa_nodes: true,
        };
        let (source, destination) = Relocator::between_threads().source(false).relocate(&mut relocation);

        assert!(!relocation.had_source);
        assert!(!relocation.crossed_numa_nodes);
        assert!(source.is_none());
        assert_eq!(destination.id(), thread::current().id());
    }

    #[test]
    fn relocator_can_be_reused() {
        let relocator = Relocator::between_threads();
        let mut first = Relocation {
            had_source: false,
            crossed_numa_nodes: true,
        };
        let mut second = Relocation {
            had_source: false,
            crossed_numa_nodes: true,
        };

        let first_coordinates = relocator.relocate(&mut first);
        let second_coordinates = relocator.relocate(&mut second);

        assert_eq!(first_coordinates, second_coordinates);
    }

    #[test]
    fn separate_relocators_share_owner() {
        let (first_source, first_destination) = Relocator::between_threads().relocate(&mut ());
        let (second_source, second_destination) = Relocator::between_numa_nodes().relocate(&mut ());

        assert_eq!(first_source.unwrap().owner(), second_source.unwrap().owner());
        assert_eq!(first_destination.owner(), second_destination.owner());
    }

    #[test]
    fn different_owner_uses_a_distinct_destination_owner() {
        let mut relocation = Relocation {
            had_source: false,
            crossed_numa_nodes: true,
        };

        let (source, destination) = Relocator::between_threads().different_owner().relocate(&mut relocation);

        assert_ne!(source.unwrap().owner(), destination.owner());
    }
}
