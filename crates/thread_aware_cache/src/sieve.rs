// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! SIEVE eviction data structures.
//!
//! This module contains the metadata nodes and list management for the SIEVE eviction policy.
//! We decouple value storage from eviction metadata to improve cache locality during eviction scans.

// These truncations are intentional - NodeIndex is u32, which is sufficient for cache capacities
#![expect(
    clippy::cast_possible_truncation,
    reason = "NodeIndex is u32, which is sufficient for expected cache capacities"
)]

use std::sync::atomic::{AtomicBool, Ordering};

/// Index type for SIEVE nodes to reduce memory footprint.
pub type NodeIndex = u32;

/// Sentinel value indicating no node (null pointer equivalent).
pub const NULL_INDEX: NodeIndex = NodeIndex::MAX;

/// Metadata node for SIEVE eviction tracking.
///
/// Each entry in the cache has a corresponding `SieveNode` that tracks eviction metadata.
/// Indices are `u32` to reduce memory footprint compared to `usize`.
///
/// The node stores a clone of the key to enable O(1) lookup during eviction, avoiding
/// the need to iterate through the entire map to find the key by hash.
#[derive(Debug)]
pub struct SieveNode<K> {
    /// A clone of the key, stored for O(1) map removal during eviction.
    pub key: Option<K>,

    /// The "Second Chance" bit.
    /// - Set to `true` on Access.
    /// - Checked/Cleared during Eviction.
    ///
    /// Using `AtomicBool` allows setting this with `Relaxed` ordering during reads,
    /// avoiding the need to upgrade to a write lock on cache hits.
    pub visited: AtomicBool,

    /// Index of the next node in the doubly-linked list.
    pub next: NodeIndex,

    /// Index of the previous node in the doubly-linked list.
    pub prev: NodeIndex,

    /// Indicates whether this node slot is currently in use.
    pub in_use: bool,
}

impl<K> SieveNode<K> {
    /// Creates a new uninitialized node.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            key: None,
            visited: AtomicBool::new(false),
            next: NULL_INDEX,
            prev: NULL_INDEX,
            in_use: false,
        }
    }

    /// Initializes this node with the given key.
    pub fn init(&mut self, key: K) {
        self.key = Some(key);
        self.visited.store(false, Ordering::Relaxed);
        self.next = NULL_INDEX;
        self.prev = NULL_INDEX;
        self.in_use = true;
    }

    /// Clears this node, marking it as unused.
    pub fn clear(&mut self) {
        self.key = None;
        self.in_use = false;
        self.next = NULL_INDEX;
        self.prev = NULL_INDEX;
    }

    /// Marks this node as visited (accessed).
    pub fn mark_visited(&self) {
        self.visited.store(true, Ordering::Relaxed);
    }

    /// Checks if this node was visited and clears the flag.
    ///
    /// Returns `true` if the node was visited.
    pub fn check_and_clear_visited(&self) -> bool {
        self.visited.swap(false, Ordering::Relaxed)
    }
}

impl<K> Default for SieveNode<K> {
    fn default() -> Self {
        Self::new()
    }
}

/// Manages the SIEVE eviction state for a shard.
#[derive(Debug)]
pub struct SieveList<K> {
    /// Pre-allocated slab of metadata nodes.
    nodes: Vec<SieveNode<K>>,

    /// The "Clock Hand" cursor for eviction.
    hand: Option<NodeIndex>,

    /// Head of the linked list (most recently inserted).
    head: Option<NodeIndex>,

    /// Tail of the linked list (oldest).
    tail: Option<NodeIndex>,

    /// Free list head for recycling node slots.
    free_head: Option<NodeIndex>,

    /// Current number of entries in the list.
    len: usize,

    /// Maximum capacity.
    capacity: usize,
}

impl<K> SieveList<K> {
    /// Creates a new SIEVE list with the given capacity.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let mut nodes = Vec::with_capacity(capacity);
        nodes.resize_with(capacity, SieveNode::new);

        // Initialize free list by chaining all nodes together
        for (i, node) in nodes.iter_mut().enumerate().take(capacity) {
            if let Some(next) = (i + 1 < capacity).then(|| (i + 1) as NodeIndex) {
                node.next = next;
            }
        }

        let free_head = (capacity > 0).then_some(0);

        Self {
            nodes,
            hand: None,
            head: None,
            tail: None,
            free_head,
            len: 0,
            capacity,
        }
    }

    /// Returns the current number of entries.
    ///
    /// This method is primarily useful for testing and debugging.
    #[cfg(test)]
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the list is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns `true` if the list is at capacity.
    #[must_use]
    pub const fn is_full(&self) -> bool {
        self.len >= self.capacity
    }

    /// Returns the capacity.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Allocates a new node slot from the free list.
    ///
    /// Returns `None` if no slots are available.
    fn alloc_node(&mut self) -> Option<NodeIndex> {
        let idx = self.free_head?;
        let next_free = self.nodes[idx as usize].next;
        self.free_head = if next_free == NULL_INDEX { None } else { Some(next_free) };
        Some(idx)
    }

    /// Returns a node slot to the free list.
    fn free_node(&mut self, idx: NodeIndex) {
        self.nodes[idx as usize].clear();
        self.nodes[idx as usize].next = self.free_head.unwrap_or(NULL_INDEX);
        self.free_head = Some(idx);
    }

    /// Inserts a new entry at the head of the list.
    ///
    /// Returns the node index if successful, or `None` if the list is full.
    pub fn insert(&mut self, key: K) -> Option<NodeIndex> {
        let idx = self.alloc_node()?;
        self.nodes[idx as usize].init(key);

        // Insert at head
        if let Some(old_head) = self.head {
            self.nodes[idx as usize].next = old_head;
            self.nodes[old_head as usize].prev = idx;
        }
        self.head = Some(idx);

        if self.tail.is_none() {
            self.tail = Some(idx);
        }

        // Initialize hand if this is the first entry
        if self.hand.is_none() {
            self.hand = Some(idx);
        }

        self.len += 1;
        Some(idx)
    }

    /// Removes a node from the list by index.
    pub fn remove(&mut self, idx: NodeIndex) {
        let node = &self.nodes[idx as usize];
        if !node.in_use {
            return;
        }

        let prev = node.prev;
        let next = node.next;

        // Update neighbors
        if prev == NULL_INDEX {
            // This was the head
            self.head = if next == NULL_INDEX { None } else { Some(next) };
        } else {
            self.nodes[prev as usize].next = next;
        }

        if next == NULL_INDEX {
            // This was the tail
            self.tail = if prev == NULL_INDEX { None } else { Some(prev) };
        } else {
            self.nodes[next as usize].prev = prev;
        }

        // Update hand if it pointed to this node
        if self.hand == Some(idx) {
            self.hand = if prev != NULL_INDEX {
                Some(prev)
            } else if next != NULL_INDEX {
                Some(next)
            } else {
                None
            };
        }

        self.free_node(idx);
        self.len -= 1;
    }

    /// Marks a node as visited.
    pub fn mark_visited(&self, idx: NodeIndex) {
        if (idx as usize) < self.nodes.len() {
            self.nodes[idx as usize].mark_visited();
        }
    }

    /// Finds and removes a victim for eviction using the SIEVE algorithm.
    ///
    /// Returns the key of the evicted entry for O(1) map removal.
    pub fn evict(&mut self) -> Option<K> {
        if self.is_empty() {
            return None;
        }

        let start = self.hand?;
        let mut cursor = start;
        let mut iterations = 0;
        let max_iterations = self.len * 2; // Safety limit to prevent infinite loops

        loop {
            if iterations >= max_iterations {
                // Fallback: just evict the current cursor position
                break;
            }
            iterations += 1;

            let node = &self.nodes[cursor as usize];
            if !node.in_use {
                // Skip unused nodes (shouldn't happen normally)
                cursor = if node.prev == NULL_INDEX {
                    self.tail.unwrap_or(start)
                } else {
                    node.prev
                };
                continue;
            }

            if node.check_and_clear_visited() {
                // Node was visited, give it a second chance
                cursor = if node.prev == NULL_INDEX {
                    // Wrap around to tail
                    self.tail.unwrap_or(cursor)
                } else {
                    node.prev
                };
            } else {
                // Found a victim
                break;
            }
        }

        // Take the key from the node before removing
        let key = self.nodes[cursor as usize].key.take();

        // Move hand to the previous position before removing
        let prev = self.nodes[cursor as usize].prev;
        self.hand = if prev == NULL_INDEX { self.tail } else { Some(prev) };

        self.remove(cursor);

        key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sieve_node_lifecycle() {
        let mut node: SieveNode<i32> = SieveNode::new();
        assert!(!node.in_use);

        node.init(12345);
        assert!(node.in_use);
        assert_eq!(node.key, Some(12345));
        assert!(!node.visited.load(Ordering::Relaxed));

        node.mark_visited();
        assert!(node.visited.load(Ordering::Relaxed));

        assert!(node.check_and_clear_visited());
        assert!(!node.visited.load(Ordering::Relaxed));

        node.clear();
        assert!(!node.in_use);
        assert!(node.key.is_none());
    }

    #[test]
    fn test_sieve_list_basic() {
        let mut list: SieveList<i32> = SieveList::new(10);
        assert!(list.is_empty());
        assert!(!list.is_full());
        assert_eq!(list.capacity(), 10);

        // Insert some entries
        let idx1 = list.insert(100).expect("should insert");
        let idx2 = list.insert(200).expect("should insert");
        let idx3 = list.insert(300).expect("should insert");

        assert_eq!(list.len(), 3);
        assert!(!list.is_empty());

        // Verify keys are stored
        assert_eq!(list.nodes[idx1 as usize].key, Some(100));
        assert_eq!(list.nodes[idx2 as usize].key, Some(200));
        assert_eq!(list.nodes[idx3 as usize].key, Some(300));

        // Remove middle entry
        list.remove(idx2);
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_sieve_list_full() {
        let mut list: SieveList<i32> = SieveList::new(3);

        list.insert(1).expect("should insert");
        list.insert(2).expect("should insert");
        list.insert(3).expect("should insert");

        assert!(list.is_full());
        assert!(list.insert(4).is_none());
    }

    #[test]
    fn test_sieve_eviction() {
        let mut list: SieveList<i32> = SieveList::new(3);

        let idx1 = list.insert(100).expect("should insert");
        let idx2 = list.insert(200).expect("should insert");
        let _idx3 = list.insert(300).expect("should insert");

        // Mark idx1 and idx2 as visited
        list.mark_visited(idx1);
        list.mark_visited(idx2);

        // Evict should find idx3 (the unvisited one)
        let key = list.evict().expect("should evict");
        assert_eq!(key, 300);
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_sieve_eviction_second_chance() {
        let mut list: SieveList<i32> = SieveList::new(3);

        let idx1 = list.insert(100).expect("should insert");
        let idx2 = list.insert(200).expect("should insert");
        let idx3 = list.insert(300).expect("should insert");

        // Mark all as visited
        list.mark_visited(idx1);
        list.mark_visited(idx2);
        list.mark_visited(idx3);

        // First eviction should clear visited flags and eventually evict one
        let key = list.evict().expect("should evict");
        // One of them should be evicted after clearing visited flags
        assert!(key == 100 || key == 200 || key == 300);
        assert_eq!(list.len(), 2);
    }
}
