// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exercises the public surface from outside the crate's own test build.
//!
//! Unit tests in `src/` compile the library with `cfg(test)`, which switches on every
//! `cfg(any(test, feature = "std"))` item whatever features are selected, so they always
//! see the three-field `Thread`. This harness links the library as an ordinary dependency,
//! so under `--no-default-features` the `no_std` shape is the one under test.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use static_assertions::assert_impl_all;
use thread_aware_core::{NumaNode, Owner, Thread};

assert_impl_all!(Thread: Clone, Eq, Send, Sync);
assert_impl_all!(Owner: Clone, Eq, Send, Sync);
assert_impl_all!(NumaNode: Copy, Eq, Send, Sync);

fn hash_of<T>(value: &T) -> u64
where
    T: Hash,
{
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[test]
fn owners_keep_their_identity_and_count() {
    let first = Owner::new(4);
    let second = Owner::new(4);

    assert_ne!(first, second, "each owner takes its own identity");
    assert_eq!(first, first, "an owner equals itself");
    assert_ne!(hash_of(&first), hash_of(&second), "distinct owners must hash apart");
    assert_eq!(first.min_threads(), 4);
}

#[test]
fn numa_nodes_compare_and_hash_by_value() {
    assert_eq!(NumaNode::new(3), NumaNode::new(3));
    assert_ne!(NumaNode::new(3), NumaNode::new(4));
    assert_eq!(
        hash_of(&NumaNode::new(3)),
        hash_of(&NumaNode::new(3)),
        "equal nodes must hash equally"
    );
}

/// Without `std` the thread id is gone, so equality and hashing cover the owner and the
/// NUMA node alone. `Thread` cannot be constructed here, which is exactly why that
/// narrowing is unobservable in a real program, so pin the shape by size instead: anything
/// wider than the two remaining fields means the id came back.
#[cfg(not(feature = "std"))]
#[test]
fn without_std_a_thread_carries_no_thread_id() {
    let align = align_of::<Thread>();
    let expected = (size_of::<Owner>() + size_of::<NumaNode>()).div_ceil(align) * align;

    assert_eq!(size_of::<Thread>(), expected, "a no_std `Thread` is its owner and its NUMA node");
}

#[cfg(feature = "std")]
#[test]
fn with_std_every_component_takes_part_in_equality() {
    use std::thread;

    let id = thread::current().id();
    let owner = Owner::new(1);
    let numa_node = NumaNode::new(0);
    let mine = Thread::new(owner.clone(), id, numa_node);

    assert_eq!(mine, Thread::new(owner.clone(), id, numa_node));
    assert_eq!(hash_of(&mine), hash_of(&Thread::new(owner.clone(), id, numa_node)));

    assert_ne!(mine, Thread::new(Owner::new(1), id, numa_node), "the owner counts");
    assert_ne!(mine, Thread::new(owner.clone(), id, NumaNode::new(1)), "the NUMA node counts");

    let elsewhere = thread::spawn(move || Thread::new(owner, thread::current().id(), numa_node))
        .join()
        .unwrap();

    assert_ne!(mine, elsewhere, "the thread id counts");
}
