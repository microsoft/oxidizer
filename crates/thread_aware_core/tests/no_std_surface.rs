// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exercises the public surface from outside the crate's own test build.
//!
//! Unit tests in `src/` compile the library with `cfg(test)`, which switches on every
//! `cfg(any(test, feature = "std"))` item whatever features are selected, so they always
//! see the three-field `Thread`. This harness links the library as an ordinary dependency,
//! so under `--no-default-features` the `no_std` shape is the one under test.

use std::borrow::Cow;
use std::collections::BTreeSet;
#[cfg(feature = "std")]
use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
#[cfg(feature = "std")]
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::sync::Arc;
#[cfg(feature = "std")]
use std::thread;

use static_assertions::{assert_impl_all, assert_not_impl_any};
#[cfg(feature = "std")]
use thread_aware_core::__private::v1::new_thread;
use thread_aware_core::__private::v1::{new_numa_node, new_owner};
use thread_aware_core::{NumaNode, Owner, Thread, ThreadAware};

assert_impl_all!(Thread: Clone, Eq, Send, Sync, Unpin, UnwindSafe, RefUnwindSafe);
assert_not_impl_any!(Thread: Copy);
assert_impl_all!(Owner: Clone, Eq, Send, Sync);
assert_impl_all!(NumaNode: Clone, Eq, Send, Sync);
assert_not_impl_any!(NumaNode: Copy);
assert_impl_all!(&'static str: ThreadAware);
assert_not_impl_any!(&'static i32: ThreadAware);
assert_not_impl_any!(BTreeSet<i32>: ThreadAware);
assert_not_impl_any!(Cow<'static, str>: ThreadAware);
assert_not_impl_any!(Arc<i32>: ThreadAware);
#[cfg(feature = "std")]
assert_not_impl_any!(HashSet<i32>: ThreadAware);
#[cfg(feature = "std")]
assert_impl_all!(SocketAddr: ThreadAware);
#[cfg(feature = "std")]
assert_impl_all!(SocketAddrV4: ThreadAware);
#[cfg(feature = "std")]
assert_impl_all!(SocketAddrV6: ThreadAware);

fn hash_of<T>(value: &T) -> u64
where
    T: Hash,
{
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[cfg(feature = "std")]
fn sample_threads() -> [Thread; 2] {
    let owner = new_owner();
    let id = thread::current().id();

    [
        new_thread(owner.clone(), id, new_numa_node(0)),
        new_thread(owner, id, new_numa_node(1)),
    ]
}

#[cfg(feature = "std")]
fn assert_relocation_is_noop<T>(mut value: T, source: Option<&Thread>, destination: &Thread)
where
    T: ThreadAware + Clone + PartialEq + std::fmt::Debug,
{
    let original = value.clone();

    value.relocate(source, destination);

    assert_eq!(value, original);
}

#[test]
fn owners_keep_their_identity() {
    let first = new_owner();
    let second = new_owner();

    assert_ne!(first, second, "each owner takes its own identity");
    assert_eq!(first, first, "an owner equals itself");
    assert_ne!(hash_of(&first), hash_of(&second), "distinct owners must hash apart");
}

#[test]
fn numa_nodes_compare_and_hash_by_value() {
    assert_eq!(new_numa_node(3), new_numa_node(3));
    assert_ne!(new_numa_node(3), new_numa_node(4));
    assert_eq!(
        hash_of(&new_numa_node(3)),
        hash_of(&new_numa_node(3)),
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

    assert_eq!(
        size_of::<Thread>(),
        expected,
        "the no_std coordinate contains only its owner and NUMA node"
    );
}

#[cfg(feature = "std")]
#[test]
fn with_std_every_component_takes_part_in_equality() {
    let id = thread::current().id();
    let owner = new_owner();
    let numa_node = new_numa_node(0);
    let mine = new_thread(owner.clone(), id, numa_node.clone());

    assert_eq!(mine, new_thread(owner.clone(), id, numa_node.clone()));
    assert_eq!(hash_of(&mine), hash_of(&new_thread(owner.clone(), id, numa_node.clone())));

    assert_ne!(mine, new_thread(new_owner(), id, numa_node.clone()), "the owner counts");
    assert_ne!(mine, new_thread(owner.clone(), id, new_numa_node(1)), "the NUMA node counts");

    let elsewhere = thread::spawn(move || new_thread(owner, thread::current().id(), numa_node))
        .join()
        .unwrap();

    assert_ne!(mine, elsewhere, "the thread id counts");
}

#[cfg(feature = "std")]
#[test]
fn socket_address_types_are_unchanged_by_relocation() {
    let threads = sample_threads();
    let source = &threads[0];
    let destination = &threads[1];
    let ipv4 = SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 1), 443);
    let ipv6 = SocketAddrV6::new(Ipv6Addr::LOCALHOST, 8443, 7, 11);

    assert_relocation_is_noop(ipv4, Some(source), destination);
    assert_relocation_is_noop(SocketAddr::V4(ipv4), Some(source), destination);
    assert_relocation_is_noop(ipv6, Some(source), destination);
    assert_relocation_is_noop(SocketAddr::V6(ipv6), Some(source), destination);
}
