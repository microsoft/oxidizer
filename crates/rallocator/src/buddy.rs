// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Three-entry hot caches and intrusive red-black trees per buddy size.
//! Standard collections would recursively call this allocator. Large-range
//! links reside in the pagemap, so decommitted blocks need not be touched.
//! Small metadata-range links reside in the free metadata blocks themselves.

use crate::pagemap::Map;

#[derive(Clone, Copy)]
pub(crate) enum Nodes {
    Map(Map),
    Inline,
}

impl Nodes {
    unsafe fn read(self, address: usize) -> (usize, usize, bool) {
        match self {
            // SAFETY: The address is a registered node owned by this tree.
            Self::Map(map) => unsafe { map.children(address) },
            Self::Inline => {
                let pointer = address as *const usize;
                // SAFETY: The owned inline node contains two initialized words.
                let left = unsafe { *pointer & !1 };
                // SAFETY: The second word is within that node.
                let right_pointer = unsafe { pointer.add(1) };
                // SAFETY: The second node word is initialized.
                let right = unsafe { *right_pointer };
                // SAFETY: The first word also carries the node's color.
                let red = unsafe { *pointer & 1 != 0 };
                (left, right, red)
            }
        }
    }

    unsafe fn write(self, address: usize, left: usize, right: usize, red: bool) {
        match self {
            // SAFETY: This registered node and its aligned links belong to the tree.
            Self::Map(map) => unsafe { map.set_children(address, left, right, red) },
            Self::Inline => {
                let pointer = address as *mut usize;
                // SAFETY: The node exclusively owns two writable metadata words.
                unsafe { *pointer = left | usize::from(red) };
                // SAFETY: The second word lies within the node.
                let right_pointer = unsafe { pointer.add(1) };
                // SAFETY: The second word is exclusively writable.
                unsafe { *right_pointer = right };
            }
        }
    }

    unsafe fn red(self, address: usize) -> bool {
        // SAFETY: Nonzero addresses are valid nodes of the current tree.
        address != 0 && unsafe { self.read(address).2 }
    }

    unsafe fn color(self, address: usize, red: bool) {
        if address != 0 {
            // SAFETY: This nonzero address belongs to the exclusively owned tree.
            let (left, right, _) = unsafe { self.read(address) };
            // SAFETY: Recoloring preserves the initialized node's links.
            unsafe { self.write(address, left, right, red) };
        }
    }

    unsafe fn rotate_left(self, root: usize) -> usize {
        // SAFETY: LLRB rotation preconditions guarantee a nonzero right child.
        let (left, pivot, color) = unsafe { self.read(root) };
        // SAFETY: The right child is a live node in this exclusive tree.
        let (middle, right, _) = unsafe { self.read(pivot) };
        // SAFETY: Rotation transfers the pivot's left subtree to the old root.
        unsafe { self.write(root, left, middle, true) };
        // SAFETY: The pivot receives the old root and its original right subtree.
        unsafe { self.write(pivot, root, right, color) };
        pivot
    }

    unsafe fn rotate_right(self, root: usize) -> usize {
        // SAFETY: LLRB rotation preconditions guarantee a nonzero left child.
        let (pivot, right, color) = unsafe { self.read(root) };
        // SAFETY: The left child is a live node in this exclusive tree.
        let (left, middle, _) = unsafe { self.read(pivot) };
        // SAFETY: Rotation transfers the pivot's right subtree to the old root.
        unsafe { self.write(root, middle, right, true) };
        // SAFETY: The pivot receives the old root and its original left subtree.
        unsafe { self.write(pivot, left, root, color) };
        pivot
    }

    unsafe fn flip(self, h: usize) {
        // SAFETY: h is nonzero; color accepts null children as black leaves.
        let (l, r, c) = unsafe { self.read(h) };
        // SAFETY: Recoloring leaves the owned root's links intact.
        unsafe { self.color(h, !c) };
        if l != 0 {
            // SAFETY: This child belongs to the exclusive tree.
            let red = unsafe { self.red(l) };
            // SAFETY: Only the child's color changes.
            unsafe { self.color(l, !red) };
        }
        if r != 0 {
            // SAFETY: This child belongs to the exclusive tree.
            let red = unsafe { self.red(r) };
            // SAFETY: Only the child's color changes.
            unsafe { self.color(r, !red) };
        }
    }

    unsafe fn balance(self, mut h: usize) -> usize {
        // SAFETY: Standard left-leaning RB transformations preserve node ownership.
        let (l, r, _) = unsafe { self.read(h) };
        // SAFETY: Both links are owned nodes or null leaves.
        if unsafe { self.red(r) } && !unsafe { self.red(l) } {
            // SAFETY: The red right child satisfies left-rotation preconditions.
            h = unsafe { self.rotate_left(h) };
        }
        // SAFETY: The possibly rotated root is still a live tree node.
        let l = unsafe { self.read(h).0 };
        // SAFETY: The left link is an owned node or a null leaf.
        if unsafe { self.red(l) } && {
            // SAFETY: A red left child cannot be null.
            let child = unsafe { self.read(l).0 };
            // SAFETY: Its left link is an owned node or null.
            unsafe { self.red(child) }
        } {
            // SAFETY: Two consecutive red left links permit right rotation.
            h = unsafe { self.rotate_right(h) };
        }
        // SAFETY: Rotations preserve the root's node ownership.
        let (l, r, _) = unsafe { self.read(h) };
        // SAFETY: Both child links belong to the exclusive tree.
        if unsafe { self.red(l) } && unsafe { self.red(r) } {
            // SAFETY: Both children are red, satisfying the color-flip precondition.
            unsafe { self.flip(h) };
        }
        h
    }

    unsafe fn insert(self, h: usize, address: usize) -> usize {
        if h == 0 {
            // SAFETY: The new block exclusively owns its node storage.
            unsafe { self.write(address, 0, 0, true) };
            return address;
        }
        // SAFETY: The nonempty subtree is initialized and exclusively owned.
        let (mut l, mut r, c) = unsafe { self.read(h) };
        if address < h {
            // SAFETY: The distinct key belongs in the left subtree.
            l = unsafe { self.insert(l, address) };
        } else {
            // SAFETY: The distinct key belongs in the right subtree.
            r = unsafe { self.insert(r, address) };
        }
        // SAFETY: The updated children remain owned by this root.
        unsafe { self.write(h, l, r, c) };
        // SAFETY: Insertion leaves a valid LLRB rebalancing input.
        unsafe { self.balance(h) }
    }

    unsafe fn move_red_left(self, mut h: usize) -> usize {
        // SAFETY: Deletion algorithm enters a 2-node with its sibling present.
        unsafe { self.flip(h) };
        // SAFETY: Color changes leave this live root and its links intact.
        let (l, r, c) = unsafe { self.read(h) };
        if r != 0 && {
            // SAFETY: The nonnull right child is an owned node.
            let child = unsafe { self.read(r).0 };
            // SAFETY: Its left link belongs to the same tree.
            unsafe { self.red(child) }
        } {
            // SAFETY: The red left grandchild permits this rotation.
            let r = unsafe { self.rotate_right(r) };
            // SAFETY: The rotated subtree replaces the root's right link.
            unsafe { self.write(h, l, r, c) };
            // SAFETY: The nonempty right subtree permits left rotation.
            h = unsafe { self.rotate_left(h) };
            // SAFETY: The deletion transformation requires this color flip.
            unsafe { self.flip(h) };
        }
        h
    }

    unsafe fn move_red_right(self, mut h: usize) -> usize {
        // SAFETY: Deletion algorithm enters a 2-node with its sibling present.
        unsafe { self.flip(h) };
        // SAFETY: The root remains initialized and owned.
        let l = unsafe { self.read(h).0 };
        if l != 0 && {
            // SAFETY: This nonnull left child belongs to the tree.
            let child = unsafe { self.read(l).0 };
            // SAFETY: Its left link is an owned node or null.
            unsafe { self.red(child) }
        } {
            // SAFETY: The red grandchild permits the deletion rotation.
            h = unsafe { self.rotate_right(h) };
            // SAFETY: The rotated deletion state satisfies flip preconditions.
            unsafe { self.flip(h) };
        }
        h
    }

    unsafe fn delete_min(self, mut h: usize) -> (usize, usize) {
        // SAFETY: h is a nonempty exclusively owned tree; rotations preserve it.
        let l = unsafe { self.read(h).0 };
        if l == 0 {
            return (0, h);
        }
        // SAFETY: l is a nonnull child of the exclusive tree.
        if !unsafe { self.red(l) } && {
            // SAFETY: The left child remains nonnull.
            let child = unsafe { self.read(l).0 };
            // SAFETY: Its left link is an owned node or null.
            !unsafe { self.red(child) }
        } {
            // SAFETY: A black 2-node needs the standard leftward red transfer.
            h = unsafe { self.move_red_left(h) };
        }
        // SAFETY: The transformed root remains owned and initialized.
        let (l, r, c) = unsafe { self.read(h) };
        // SAFETY: The nonempty left subtree still contains its minimum.
        let (l, removed) = unsafe { self.delete_min(l) };
        // SAFETY: Relinking removes only the returned node.
        unsafe { self.write(h, l, r, c) };
        // SAFETY: Deletion leaves a valid rebalancing input.
        (unsafe { self.balance(h) }, removed)
    }

    unsafe fn delete(self, mut h: usize, key: usize) -> usize {
        if key < h {
            // SAFETY: The present key lies in this live root's left subtree.
            let l = unsafe { self.read(h).0 };
            // SAFETY: The left child contains the key and cannot be null.
            if !unsafe { self.red(l) } && {
                // SAFETY: The left child remains a live node.
                let child = unsafe { self.read(l).0 };
                // SAFETY: Its left link is a node or null leaf.
                !unsafe { self.red(child) }
            } {
                // SAFETY: The black 2-node needs a leftward red transfer.
                h = unsafe { self.move_red_left(h) };
            }
            // SAFETY: Transformation preserves this owned root.
            let (l, r, c) = unsafe { self.read(h) };
            // SAFETY: The left subtree still contains the key.
            let l = unsafe { self.delete(l, key) };
            // SAFETY: The updated children belong to this root.
            unsafe { self.write(h, l, r, c) };
        } else {
            // SAFETY: The live root contains the key.
            let left = unsafe { self.read(h).0 };
            // SAFETY: Its left link is an owned node or null.
            if unsafe { self.red(left) } {
                // SAFETY: The red left child permits right rotation.
                h = unsafe { self.rotate_right(h) };
            }
            // SAFETY: The possibly rotated root remains initialized.
            if key == h && unsafe { self.read(h).1 } == 0 {
                return 0;
            }
            // SAFETY: The right subtree exists on this deletion path.
            let r = unsafe { self.read(h).1 };
            // SAFETY: The right child is a live tree node.
            if !unsafe { self.red(r) } && {
                // SAFETY: The right child cannot be null on this path.
                let child = unsafe { self.read(r).0 };
                // SAFETY: Its left link is an owned node or null.
                !unsafe { self.red(child) }
            } {
                // SAFETY: The black 2-node needs a rightward red transfer.
                h = unsafe { self.move_red_right(h) };
            }
            // SAFETY: The transformed root remains live and owned.
            let (l, r, c) = unsafe { self.read(h) };
            if key == h {
                // Intrusive nodes cannot copy a key; transplant the successor.
                // SAFETY: The right subtree is nonempty.
                let (r, successor) = unsafe { self.delete_min(r) };
                // SAFETY: The removed successor is now exclusively available.
                unsafe { self.write(successor, l, r, c) };
                h = successor;
            } else {
                // SAFETY: The right subtree still contains the key.
                let r = unsafe { self.delete(r, key) };
                // SAFETY: The remaining children are owned by this root.
                unsafe { self.write(h, l, r, c) };
            }
        }
        // SAFETY: Deletion leaves a valid rebalancing input.
        unsafe { self.balance(h) }
    }

    unsafe fn contains(self, mut h: usize, key: usize) -> bool {
        while h != 0 {
            if h == key {
                return true;
            }
            // SAFETY: Every traversed node belongs to the exclusive tree.
            let (l, r, _) = unsafe { self.read(h) };
            h = if key < h { l } else { r };
        }
        false
    }

    unsafe fn remove(self, root: &mut usize, key: usize) {
        // SAFETY: key is present; the mutable root grants exclusive tree access.
        let (l, r, _) = unsafe { self.read(*root) };
        // SAFETY: Root links refer to owned nodes or null leaves.
        if !unsafe { self.red(l) } && !unsafe { self.red(r) } {
            // SAFETY: The deletion algorithm temporarily reddens this owned root.
            unsafe { self.color(*root, true) };
        }
        // SAFETY: The key is present in the exclusively owned tree.
        *root = unsafe { self.delete(*root, key) };
        // SAFETY: The resulting root is a live node or null.
        unsafe { self.color(*root, false) };
    }

    unsafe fn smallest(self, root: &mut usize) -> usize {
        let mut h = *root;
        if h == 0 {
            return 0;
        }
        // SAFETY: Each traversed node is initialized and exclusively owned.
        while unsafe { self.read(h).0 } != 0 {
            // SAFETY: The loop condition established a nonnull left child.
            h = unsafe { self.read(h).0 };
        }
        // SAFETY: The located minimum is present in this tree.
        unsafe { self.remove(root, h) };
        h
    }
}

#[derive(Clone, Copy)]
struct Entry {
    cache: [usize; 3],
    root: usize,
}

pub(crate) struct Buddy<const MIN: usize, const MAX: usize> {
    // Const-generic subtraction is not stable. Unused slots are never accessed.
    entries: [Entry; MAX],
    empty_above: usize,
}

impl<const MIN: usize, const MAX: usize> Buddy<MIN, MAX> {
    pub(crate) const fn new() -> Self {
        Self {
            entries: [Entry { cache: [0; 3], root: 0 }; MAX],
            empty_above: MIN,
        }
    }
}

impl<const MIN: usize, const MAX: usize> Buddy<MIN, MAX> {
    /// # Safety
    /// The caller excludes mutation of this buddy and all of its owned nodes.
    pub(crate) unsafe fn observe(&self, nodes: Nodes, budget: &mut usize) -> seismograph_rallocator::native::Ranges {
        let mut result = seismograph_rallocator::native::Ranges::EMPTY;
        for bits in MIN..self.empty_above.min(MAX) {
            let entry = &self.entries[bits];
            result.counts[bits] = entry.cache.iter().filter(|address| **address != 0).count() as u64;
            let mut pending = [0usize; 64];
            let mut length = usize::from(entry.root != 0);
            pending[0] = entry.root;
            while length != 0 {
                if *budget == 0 {
                    result.complete = false;
                    break;
                }
                *budget -= 1;
                length -= 1;
                let address = pending[length];
                result.counts[bits] += 1;
                // SAFETY: Nodes stay live and immutable under the caller's owner lease or global lock.
                let (left, right, _) = unsafe { nodes.read(address) };
                for child in [left, right] {
                    if child != 0 {
                        if length == pending.len() {
                            result.complete = false;
                            break;
                        }
                        pending[length] = child;
                        length += 1;
                    }
                }
                if !result.complete {
                    break;
                }
            }
        }
        result
    }

    /// # Safety
    /// The aligned, power-of-two block is exclusively owned and registered (Map)
    /// or committed (Inline), and is not already in this or any other buddy.
    /// All operations on a given buddy and its nodes must be mutually exclusive.
    pub(crate) unsafe fn add(&mut self, nodes: Nodes, mut address: usize, mut bits: usize) -> usize {
        while bits < MAX {
            let sibling = address ^ (1usize << bits);
            let entry = &mut self.entries[bits];
            let cached = entry.cache.iter().position(|&value| value == sibling);
            // SAFETY: Every cached tree node belongs to this exclusive buddy.
            let present = cached.is_some() || unsafe { nodes.contains(entry.root, sibling) };
            let merge = present
                && match nodes {
                    // SAFETY: Presence proves both buddies are registered and owned.
                    Nodes::Map(map) => unsafe { map.can_merge(address, 1 << bits) },
                    Nodes::Inline => true,
                };
            if merge {
                if let Some(i) = cached {
                    // SAFETY: The owned tree supplies a replacement for the merged cache entry.
                    entry.cache[i] = unsafe { nodes.smallest(&mut entry.root) };
                } else {
                    // SAFETY: The sibling was found in this tree.
                    unsafe { nodes.remove(&mut entry.root, sibling) };
                }
                address &= !(1usize << bits);
                bits += 1;
                continue;
            }
            self.empty_above = self.empty_above.max(bits + 1);
            if let Some(slot) = entry.cache.iter_mut().find(|slot| **slot == 0) {
                *slot = address;
            } else {
                // SAFETY: The incoming block is distinct from all cached nodes.
                entry.root = unsafe { nodes.insert(entry.root, address) };
                // SAFETY: Insertion returned an owned, nonempty root.
                unsafe { nodes.color(entry.root, false) };
            }
            return 0;
        }
        address
    }

    /// # Safety
    /// All nodes in this buddy remain registered / committed as appropriate.
    /// `bits` is in MIN..MAX, and caller has exclusive access to this range.
    pub(crate) unsafe fn take(&mut self, nodes: Nodes, bits: usize) -> usize {
        if bits >= self.empty_above || bits >= MAX {
            return 0;
        }
        let entry = &mut self.entries[bits];
        // SAFETY: The caller exclusively owns this initialized tree.
        let mut address = unsafe { nodes.smallest(&mut entry.root) };
        // Preserve buddy.h's selection AND its cache-replacement order.
        for slot in &mut entry.cache {
            if address == 0 || *slot > address {
                std::mem::swap(slot, &mut address);
            }
        }
        if address != 0 {
            return address;
        }
        if bits + 1 == MAX {
            return 0;
        }
        // SAFETY: The next class remains within the same exclusive buddy.
        let larger = unsafe { self.take(nodes, bits + 1) };
        if larger == 0 {
            self.empty_above = bits;
            return 0;
        }
        // SAFETY: The upper half is disjoint and retained by this buddy.
        let overflow = unsafe { self.add(nodes, larger + (1 << bits), bits) };
        debug_assert_eq!(overflow, 0);
        larger
    }

    #[cfg(test)]
    pub(crate) unsafe fn validate(&self, nodes: Nodes) -> Vec<(usize, usize)> {
        unsafe fn visit(nodes: Nodes, address: usize, low: usize, high: usize, bits: usize, blocks: &mut Vec<(usize, usize)>) -> usize {
            if address == 0 {
                return 1;
            }

            assert!(low < address && address < high, "buddy tree is not ordered at {address:#x}");
            assert_eq!(address % (1usize << bits), 0, "misaligned buddy block at {address:#x}");
            // SAFETY: The caller holds exclusive access to the buddy and all
            // tree nodes remain backed for the duration of this traversal.
            let (left, right, red) = unsafe { nodes.read(address) };
            // SAFETY: A nonzero right link is another node in this tree.
            assert!(!unsafe { nodes.red(right) }, "right-leaning red link at {address:#x}");
            if red {
                // SAFETY: Child links are either null or initialized tree nodes.
                let left_red = unsafe { nodes.red(left) };
                // SAFETY: The right link has the same tree-node contract.
                let right_red = unsafe { nodes.red(right) };
                assert!(!left_red && !right_red, "consecutive red links at {address:#x}");
            }
            blocks.push((address, bits));
            // SAFETY: The two subtrees are disjoint and retain the same backing.
            let left_height = unsafe { visit(nodes, left, low, address, bits, blocks) };
            // SAFETY: The right subtree is likewise owned by this buddy.
            let right_height = unsafe { visit(nodes, right, address, high, bits, blocks) };
            assert_eq!(left_height, right_height, "red-black height mismatch at {address:#x}");
            left_height + usize::from(!red)
        }

        let mut blocks = Vec::new();
        let mut highest = MIN;
        for bits in MIN..MAX {
            let entry = &self.entries[bits];
            for &address in &entry.cache {
                if address != 0 {
                    assert_eq!(address % (1usize << bits), 0, "misaligned cached buddy block at {address:#x}");
                    blocks.push((address, bits));
                    highest = highest.max(bits + 1);
                }
            }
            if entry.root != 0 {
                // SAFETY: The root belongs to this exclusively inspected buddy.
                assert!(!unsafe { nodes.red(entry.root) }, "buddy tree root must be black");
                // SAFETY: The complete tree remains backed and immutable here.
                unsafe { visit(nodes, entry.root, 0, usize::MAX, bits, &mut blocks) };
                highest = highest.max(bits + 1);
            }
        }
        assert!(
            self.empty_above >= highest && self.empty_above <= MAX,
            "buddy empty_above excludes an occupied class"
        );
        blocks.sort_unstable();
        for pair in blocks.windows(2) {
            let (left, left_bits) = pair[0];
            let (right, _) = pair[1];
            assert!(
                left + (1usize << left_bits) <= right,
                "overlapping buddy blocks at {left:#x} and {right:#x}"
            );
        }
        blocks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hal;

    #[test]
    fn bounded_observation_counts_cached_and_tree_ranges_without_mutation() {
        use std::alloc::{GlobalAlloc, Layout, System};

        let layout = Layout::from_size_align(4096, 4096).unwrap();
        // SAFETY: The layout provides aligned writable inline buddy nodes.
        let pointer = unsafe { System.alloc(layout) };
        assert!(!pointer.is_null());
        let mut buddy = Buddy::<4, 12>::new();
        for index in 0..6 {
            // SAFETY: Disjoint aligned 16-byte nodes occupy one half of each 32-byte pair.
            assert_eq!(unsafe { buddy.add(Nodes::Inline, pointer.addr() + index * 32, 4) }, 0);
        }
        let mut budget = 16;
        // SAFETY: No mutation occurs during this observation of exclusively owned inline nodes.
        let observed = unsafe { buddy.observe(Nodes::Inline, &mut budget) };
        assert!(observed.complete);
        assert_eq!(observed.counts[4], 6);
        let mut exhausted = 0;
        // SAFETY: The unchanged buddy still exclusively owns every initialized node.
        let partial = unsafe { buddy.observe(Nodes::Inline, &mut exhausted) };
        assert!(!partial.complete);
        assert_eq!(partial.counts[4], 3);
        // SAFETY: The initialized nodes are exclusively owned until backing cleanup.
        assert_eq!(unsafe { buddy.validate(Nodes::Inline) }.len(), 6);
        // SAFETY: No buddy node is accessed after freeing the exact System backing.
        unsafe { System.dealloc(pointer, layout) };
    }

    fn shuffle(values: &mut [usize]) {
        let mut seed = 0x0010_2938_4756_u64;
        for i in (1..values.len()).rev() {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            values.swap(i, usize::try_from(seed).unwrap() % (i + 1));
        }
    }

    unsafe fn validate(nodes: Nodes, h: usize, low: usize, high: usize) -> usize {
        if h == 0 {
            return 1;
        }
        assert!(low < h && h < high);
        // SAFETY: Test traverses the initialized tree inside its live backing reservation.
        let (l, r, red) = unsafe { nodes.read(h) };
        // SAFETY: The right link belongs to the same initialized test tree.
        assert!(!unsafe { nodes.red(r) }, "LLRB right link must be black");
        if red {
            // SAFETY: Both links are initialized nodes or null leaves.
            assert!(!unsafe { nodes.red(l) } && !unsafe { nodes.red(r) });
        }
        // SAFETY: Recursive traversal retains the complete backing reservation.
        let left = unsafe { validate(nodes, l, low, h) };
        // SAFETY: The disjoint right subtree has the same live backing.
        let right = unsafe { validate(nodes, r, h, high) };
        assert_eq!(left, right, "red-black height mismatch at {h:#x}");
        left + usize::from(!red)
    }

    #[test]
    fn red_black_insertion_and_arbitrary_removal() {
        let base = hal::reserve(1 << 18, 1 << 18) as usize;
        assert_ne!(base, 0);
        // SAFETY: Entire reservation is exclusively owned by this test.
        assert!(unsafe { hal::commit(base as *mut u8, 1 << 18) });
        let nodes = Nodes::Inline;
        let mut root = 0;
        let mut addresses = (0..4000).map(|i| base + i * 32).collect::<Vec<_>>();
        shuffle(&mut addresses);
        for &address in &addresses {
            // SAFETY: Each distinct aligned block is transferred once.
            root = unsafe { nodes.insert(root, address) };
            // SAFETY: Insertion returned an exclusively owned initialized root.
            unsafe { nodes.color(root, false) };
        }
        // SAFETY: The entire initialized tree remains exclusively owned.
        unsafe { validate(nodes, root, 0, usize::MAX) };
        shuffle(&mut addresses);
        for (i, &address) in addresses.iter().enumerate() {
            // SAFETY: Every traversed node remains within the live reservation.
            assert!(unsafe { nodes.contains(root, address) });
            // SAFETY: The distinct key was just confirmed present.
            unsafe { nodes.remove(&mut root, address) };
            if i % 37 == 0 {
                // SAFETY: Removal preserves the remaining nodes' backing.
                unsafe { validate(nodes, root, 0, usize::MAX) };
            }
        }
        assert_eq!(root, 0);
        // SAFETY: All tree nodes have been removed.
        unsafe { hal::release(base as *mut u8, 1 << 18) };
    }

    #[test]
    fn tree_minimum_precedes_hot_cache_comparison() {
        let base = hal::reserve(1 << 16, 1 << 16) as usize;
        assert_ne!(base, 0);
        // SAFETY: Test owns all aligned blocks and retires buddy before release.
        assert!(unsafe { hal::commit(base as *mut u8, 1 << 16) });
        let mut buddy = Buddy::<4, 16>::new();
        for offset in [0, 32, 64, 96, 128, 160] {
            // SAFETY: Each disjoint block is committed and exclusively transferred.
            assert_eq!(unsafe { buddy.add(Nodes::Inline, base + offset, 4) }, 0);
        }
        for offset in [96, 128, 160, 64, 32, 0] {
            // SAFETY: The buddy exclusively owns all remaining nodes.
            assert_eq!(unsafe { buddy.take(Nodes::Inline, 4) }, base + offset);
        }
        // SAFETY: This buddy is empty and still exclusively owned.
        assert_eq!(unsafe { buddy.take(Nodes::Inline, 4) }, 0);
        // SAFETY: No node remains in the reservation.
        unsafe { hal::release(base as *mut u8, 1 << 16) };
    }

    #[test]
    fn merged_hot_entry_is_replenished_from_tree_minimum() {
        let base = hal::reserve(1 << 16, 1 << 16) as usize;
        assert_ne!(base, 0);
        // SAFETY: Test owns the committed reservation and its disjoint blocks.
        assert!(unsafe { hal::commit(base as *mut u8, 1 << 16) });
        let mut buddy = Buddy::<4, 16>::new();
        for offset in [0, 32, 64, 96, 128, 160] {
            // SAFETY: Each disjoint committed block transfers exactly once.
            assert_eq!(unsafe { buddy.add(Nodes::Inline, base + offset, 4) }, 0);
        }
        // Merge base with its sibling; refill cache[0] with tree minimum 96.
        // SAFETY: This new block is the distinct committed sibling of base.
        assert_eq!(unsafe { buddy.add(Nodes::Inline, base + 16, 4) }, 0);
        assert_eq!(buddy.entries[4].cache, [base + 96, base + 32, base + 64]);
        for offset in [128, 160, 96, 64, 32] {
            // SAFETY: All remaining nodes are owned by this buddy.
            assert_eq!(unsafe { buddy.take(Nodes::Inline, 4) }, base + offset);
        }
        // SAFETY: The coalesced block is still exclusively cached.
        assert_eq!(unsafe { buddy.take(Nodes::Inline, 5) }, base);
        // SAFETY: The buddy is empty.
        assert_eq!(unsafe { buddy.take(Nodes::Inline, 4) }, 0);
        // SAFETY: Every node has been removed before release.
        unsafe { hal::release(base as *mut u8, 1 << 16) };
    }

    #[test]
    fn split_and_recombine_every_small_block() {
        let base = hal::reserve(1 << 20, 1 << 20) as usize;
        assert_ne!(base, 0);
        // SAFETY: A single test owns the complete committed buddy range.
        assert!(unsafe { hal::commit(base as *mut u8, 1 << 20) });
        let nodes = Nodes::Inline;
        let mut buddy = Buddy::<4, 21>::new();
        // SAFETY: The entire committed range transfers to this empty buddy.
        assert_eq!(unsafe { buddy.add(nodes, base, 20) }, 0);
        let mut blocks = Vec::new();
        for _ in 0..(1 << 16) {
            // SAFETY: The cached range is exclusively owned and committed.
            let block = unsafe { buddy.take(nodes, 4) };
            assert!(block >= base && block < base + (1 << 20));
            blocks.push(block);
        }
        // SAFETY: All blocks have been removed.
        assert_eq!(unsafe { buddy.take(nodes, 4) }, 0);
        blocks.sort_unstable();
        assert_eq!(blocks.iter().copied().collect::<std::collections::BTreeSet<_>>().len(), 1 << 16);
        shuffle(&mut blocks);
        for block in blocks {
            // SAFETY: Each unique retired block is returned exactly once.
            assert_eq!(unsafe { buddy.add(nodes, block, 4) }, 0);
        }
        // SAFETY: The coalesced reservation remains owned by the buddy.
        assert_eq!(unsafe { buddy.take(nodes, 20) }, base);
        // SAFETY: The buddy is now empty.
        assert_eq!(unsafe { buddy.take(nodes, 4) }, 0);
        // SAFETY: No node retains access to this reservation.
        unsafe { hal::release(base as *mut u8, 1 << 20) };
    }
}
