// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arena-backed [`hashbrown`] collection builders on [`Arena`].
//!
//! Gated on the `hashbrown` feature. `&Arena` implements `hashbrown`'s
//! allocator trait (see `allocator_impl`), so these helpers simply hand the
//! arena to `hashbrown`'s `*_in` constructors. The returned collections borrow
//! the arena for their lifetime and use `hashbrown`'s
//! [`DefaultHashBuilder`](hashbrown::DefaultHashBuilder).

use core::hash::Hash;

use allocator_api2::alloc::Allocator;
use hashbrown::{DefaultHashBuilder, HashMap, HashSet};

use super::Arena;

impl<A: Allocator + Clone> Arena<A> {
    /// Create a new, empty [`hashbrown::HashMap`] backed by this arena. No
    /// allocation is performed until the first insertion.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let mut map = arena.alloc_hash_map::<u32, &str>();
    /// map.insert(1, "one");
    /// assert_eq!(map.get(&1), Some(&"one"));
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_hash_map<K, V>(&self) -> HashMap<K, V, DefaultHashBuilder, &Self> {
        HashMap::new_in(self)
    }

    /// Create an arena-backed [`hashbrown::HashMap`] with the requested capacity.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let mut map = arena.alloc_hash_map_with_capacity::<u32, u32>(16);
    /// map.insert(1, 10);
    /// assert!(map.capacity() >= 16);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_hash_map_with_capacity<K, V>(&self, capacity: usize) -> HashMap<K, V, DefaultHashBuilder, &Self> {
        HashMap::with_capacity_in(capacity, self)
    }

    /// Create a new, empty [`hashbrown::HashSet`] backed by this arena. No
    /// allocation is performed until the first insertion.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let mut set = arena.alloc_set::<u32>();
    /// set.insert(7);
    /// assert!(set.contains(&7));
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_set<T: Hash + Eq>(&self) -> HashSet<T, DefaultHashBuilder, &Self> {
        HashSet::new_in(self)
    }

    /// Create an arena-backed [`hashbrown::HashSet`] with the requested capacity.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let mut set = arena.alloc_set_with_capacity::<u32>(16);
    /// set.insert(7);
    /// assert!(set.capacity() >= 16);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_set_with_capacity<T: Hash + Eq>(&self, capacity: usize) -> HashSet<T, DefaultHashBuilder, &Self> {
        HashSet::with_capacity_in(capacity, self)
    }
}
