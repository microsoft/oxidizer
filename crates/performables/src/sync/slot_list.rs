// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// A VecDeque cannot remove a known waiter without scanning and shifting.
// Stable indices give O(1) unlinking in safe Rust; generations distinguish
// registrations whose slots have been reused after notification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Key {
    pub(super) index: usize,
    generation: u64,
}

#[derive(Debug)]
struct Slot<T> {
    value: Option<T>,
    previous: Option<usize>,
    next: Option<usize>,
    generation: u64,
}

#[derive(Debug)]
pub(super) struct SlotList<T> {
    slots: Vec<Slot<T>>,
    first: Option<usize>,
    last: Option<usize>,
    free: Option<usize>,
    len: usize,
}

impl<T> SlotList<T> {
    pub(super) const fn new() -> Self {
        Self {
            slots: Vec::new(),
            first: None,
            last: None,
            free: None,
            len: 0,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(super) fn push_back(&mut self, value: T) -> Key {
        let index = if let Some(index) = self.free {
            let slot = &mut self.slots[index];
            self.free = slot.next;
            slot.generation = slot.generation.wrapping_add(1);
            index
        } else {
            let index = self.slots.len();
            self.slots.push(Slot {
                value: None,
                previous: None,
                next: None,
                generation: 0,
            });
            index
        };
        let slot = &mut self.slots[index];
        slot.value = Some(value);
        slot.previous = self.last;
        slot.next = None;
        let key = Key {
            index,
            generation: slot.generation,
        };
        if let Some(last) = self.last {
            self.slots[last].next = Some(index);
        } else {
            self.first = Some(index);
        }
        self.last = Some(index);
        self.len += 1;
        key
    }

    pub(super) fn get_mut(&mut self, key: Key) -> Option<&mut T> {
        let slot = self.slots.get_mut(key.index)?;
        (slot.generation == key.generation).then_some(slot.value.as_mut()).flatten()
    }

    pub(super) fn remove(&mut self, key: Key) -> Option<T> {
        let slot = self.slots.get(key.index)?;
        if slot.generation != key.generation || slot.value.is_none() {
            return None;
        }
        let previous = slot.previous;
        let next = slot.next;
        if let Some(previous) = previous {
            self.slots[previous].next = next;
        } else {
            self.first = next;
        }
        if let Some(next) = next {
            self.slots[next].previous = previous;
        } else {
            self.last = previous;
        }
        let slot = &mut self.slots[key.index];
        slot.previous = None;
        slot.next = self.free;
        self.free = Some(key.index);
        self.len -= 1;
        slot.value.take()
    }

    pub(super) fn remove_index_if(&mut self, index: usize, matches: impl FnOnce(&T) -> bool) -> Option<T> {
        let slot = self.slots.get(index)?;
        if !matches(slot.value.as_ref()?) {
            return None;
        }
        self.remove(Key {
            index,
            generation: slot.generation,
        })
    }

    pub(super) fn pop_front(&mut self) -> Option<T> {
        let index = self.first?;
        self.remove(Key {
            index,
            generation: self.slots[index].generation,
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn unlinking_preserves_order_and_reused_slots_reject_old_keys() {
        let mut list = SlotList::new();
        assert!(list.is_empty());
        assert_eq!(list.pop_front(), None);
        let first = list.push_back(1);
        let middle = list.push_back(2);
        let tail = list.push_back(3);
        assert_eq!(list.len(), 3);
        assert_eq!(list.remove(middle), Some(2));
        let reused = list.push_back(4);
        assert_eq!(reused.index, middle.index);
        assert_ne!(reused, middle);
        assert_eq!(list.remove(middle), None);
        assert_eq!(list.get_mut(middle), None);
        *list.get_mut(reused).unwrap() = 5;
        assert_eq!(list.remove_index_if(first.index, |value| *value == 2), None);
        assert_eq!(list.pop_front(), Some(1));
        assert_eq!(list.remove(tail), Some(3));
        assert_eq!(list.pop_front(), Some(5));
        assert_eq!(list.remove(reused), None);
        assert!(list.is_empty());
        assert_eq!(
            list.get_mut(Key {
                index: usize::MAX,
                generation: 0
            }),
            None
        );
        assert_eq!(
            list.remove(Key {
                index: usize::MAX,
                generation: 0
            }),
            None
        );
        assert_eq!(list.remove_index_if(usize::MAX, |_| true), None);
        assert_eq!(list.remove_index_if(reused.index, |_| true), None);
    }
}
